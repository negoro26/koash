use crate::cell::Cell;
use crate::rows::DynRows;
use crate::{Column, Datum, Schema, StructuredDiagnostic, StructuredRecord, json};

#[derive(Debug, Clone)]
pub struct RecordBatch {
    schema: Schema,
    cells: Vec<Cell>,
    text: String,
}

/// One row of a batch, read through [`Datum`].
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    cells: &'a [Cell],
    text: &'a str,
    columns: &'a [Column],
}

impl<'a> Row<'a> {
    pub fn get(&self, col: usize) -> Datum<'a> {
        self.cells[col].datum(self.text, self.columns, col)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = Datum<'a>> + use<'a> {
        let (text, columns) = (self.text, self.columns);
        self.cells
            .iter()
            .enumerate()
            .map(move |(col, cell)| cell.datum(text, columns, col))
    }
}

/// A permutation of a batch's rows, made by [`RecordBatch::order_by`].
#[derive(Debug)]
pub struct RowOrder(Vec<usize>);

impl RecordBatch {
    pub fn empty() -> Self {
        Self::from_parts(Schema::empty(), Vec::new(), String::new())
    }

    pub(crate) fn from_parts(schema: Schema, cells: Vec<Cell>, text: String) -> Self {
        Self {
            schema,
            cells,
            text,
        }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn ncols(&self) -> usize {
        self.schema.len()
    }

    pub fn nrows(&self) -> usize {
        self.cells.len().checked_div(self.schema.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.nrows() == 0
    }

    pub fn row(&self, idx: usize) -> Row<'_> {
        let ncols = self.schema.len();
        Row {
            cells: &self.cells[idx * ncols..(idx + 1) * ncols],
            text: &self.text,
            columns: self.schema.columns(),
        }
    }

    pub fn col_index(&self, name: &str) -> Option<usize> {
        self.schema.index_of(name)
    }

    pub fn truncate_rows(&mut self, n: usize) {
        let keep = n.min(self.nrows()) * self.schema.len();
        self.cells.truncate(keep);
    }

    pub fn tail_rows(&mut self, n: usize) {
        let nrows = self.nrows();
        let drop_rows = nrows.saturating_sub(n);
        if drop_rows > 0 {
            self.cells.drain(..drop_rows * self.schema.len());
        }
    }

    pub fn rows(&self) -> impl ExactSizeIterator<Item = Row<'_>> {
        let text = self.text.as_str();
        let columns = self.schema.columns();
        self.cells
            .chunks_exact(self.schema.len().max(1))
            .map(move |cells| Row {
                cells,
                text,
                columns,
            })
    }

    /// Keeps the rows `keep` accepts, in order. Cells move inside the batch and no text is copied.
    pub fn retain_rows(&mut self, mut keep: impl FnMut(Row<'_>) -> bool) {
        let ncols = self.schema.len();
        let mut write = 0;
        for read in 0..self.nrows() {
            let from = read * ncols;
            let cells = &self.cells[from..from + ncols];
            if !keep(Row {
                cells,
                text: &self.text,
                columns: self.schema.columns(),
            }) {
                continue;
            }
            if write != read {
                self.cells.copy_within(from..from + ncols, write * ncols);
            }
            write += 1;
        }
        self.cells.truncate(write * ncols);
    }

    /// The row order that sorts by `key`, stable. `key` runs once per row.
    pub fn order_by<'s, K: Ord>(&'s self, mut key: impl FnMut(Row<'s>) -> K) -> RowOrder {
        let mut keyed: Vec<(K, usize)> = self
            .rows()
            .enumerate()
            .map(|(idx, row)| (key(row), idx))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        RowOrder(keyed.into_iter().map(|(_, idx)| idx).collect())
    }

    /// Reorders the rows. An order made for a batch of another height is ignored, because an
    /// out-of-range row would abort the shell.
    pub fn take_rows(&mut self, order: RowOrder) {
        let ncols = self.schema.len();
        if order.0.len() != self.nrows() {
            return;
        }
        let mut cells = Vec::with_capacity(self.cells.len());
        for idx in order.0 {
            cells.extend_from_slice(&self.cells[idx * ncols..(idx + 1) * ncols]);
        }
        self.cells = cells;
    }

    /// Keeps the listed columns in the listed order. A column listed twice appears twice.
    pub fn select_columns(&mut self, cols: &[usize]) {
        let ncols = self.schema.len();
        let nrows = self.nrows();
        let mut cells = Vec::with_capacity(nrows * cols.len());
        for row in self.cells.chunks_exact(ncols.max(1)).take(nrows) {
            cells.extend(cols.iter().map(|&col| row[col]));
        }
        self.schema = self.schema.project(cols);
        self.cells = cells;
    }

    pub fn from_maps(records: Vec<StructuredRecord>) -> Result<Self, StructuredDiagnostic> {
        if records.is_empty() {
            return Ok(Self::empty());
        }
        let mut names: Vec<&str> = Vec::new();
        for rec in &records {
            for k in rec.keys() {
                if !names.contains(&k.as_str()) {
                    names.push(k);
                }
            }
        }
        let schema = Schema::named(names);
        let mut rows = DynRows::new(schema.clone());
        for mut rec in records {
            rows.push_with(|out| {
                for col in schema.names() {
                    match rec.remove(col) {
                        Some(v) => json::put_value(out, v),
                        None => out.put(Datum::Null),
                    }
                }
            });
        }
        rows.finish()
    }

    /// The first `rows` rows of `batches` as one batch.
    ///
    /// The columns are the names that have a non-null cell in those rows, in the order a walk over
    /// the rows meets them, each row visited in name order. Cells follow the JSON rule: a name
    /// repeated in a batch keeps its last non-null cell. A row with no non-null cell stays as a row
    /// of nulls while any column survives, and vanishes with the columns when none does.
    pub fn merged(batches: &[RecordBatch], rows: usize) -> Self {
        let mut names: Vec<Column> = Vec::new();
        let mut left = rows;
        for batch in batches {
            let order = json::name_order(batch);
            for row in batch.rows().take(left) {
                for col in json::visible_columns(batch.schema.columns(), &order, row) {
                    let name = &batch.schema.columns()[col];
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
            left = left.saturating_sub(batch.nrows());
        }

        let schema = Schema::from(names);
        let ncols = schema.len();
        let mut out = DynRows::new(schema.clone());
        let mut cells: Vec<Datum<'_>> = Vec::new();
        let mut left = rows;
        for batch in batches {
            let order = json::name_order(batch);
            let slot: Vec<Option<usize>> = batch
                .schema
                .columns()
                .iter()
                .map(|name| schema.columns().iter().position(|n| n == name))
                .collect();
            for row in batch.rows().take(left) {
                cells.clear();
                cells.resize(ncols, Datum::Null);
                for col in json::visible_columns(batch.schema.columns(), &order, row) {
                    if let Some(at) = slot[col] {
                        cells[at] = match row.get(col) {
                            // The RPC compact path used to round-trip through JSON, which has no NaN.
                            Datum::Float(f) if !f.is_finite() => Datum::Null,
                            other => other,
                        };
                    }
                }
                out.push_with(|sink| cells.iter().for_each(|&cell| sink.put(cell)));
            }
            left = left.saturating_sub(batch.nrows());
        }
        out.into_batch()
    }

    #[cfg(test)]
    pub(crate) fn text_bytes(&self) -> usize {
        self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn batch(rows: &[serde_json::Value]) -> RecordBatch {
        let maps = rows
            .iter()
            .map(|row| serde_json::from_value(row.clone()).unwrap_or_default())
            .collect();
        RecordBatch::from_maps(maps).unwrap()
    }

    fn column(batch: &RecordBatch, name: &str) -> Vec<String> {
        let col = batch.col_index(name).unwrap_or(usize::MAX);
        let mut scratch = String::new();
        batch
            .rows()
            .map(|row| row.get(col).text(&mut scratch).to_string())
            .collect()
    }

    #[test]
    fn retain_rows_keeps_order_and_whole_rows() {
        let mut b = batch(&[
            json!({"k": 1, "v": "a"}),
            json!({"k": 2, "v": "b"}),
            json!({"k": 3, "v": "c"}),
            json!({"k": 4, "v": "d"}),
        ]);
        let k = b.col_index("k").unwrap_or(0);
        b.retain_rows(|row| row.get(k).as_f64().is_some_and(|n| n != 2.0 && n != 3.0));
        assert_eq!(column(&b, "k"), ["1", "4"]);
        assert_eq!(column(&b, "v"), ["a", "d"]);
        b.retain_rows(|_| false);
        assert_eq!((b.nrows(), b.ncols()), (0, 2));
    }

    #[test]
    fn order_by_is_stable_and_take_rows_moves_whole_rows() {
        let mut b = batch(&[
            json!({"g": "x", "n": 1}),
            json!({"g": "y", "n": 2}),
            json!({"g": "x", "n": 3}),
            json!({"g": "y", "n": 4}),
        ]);
        let g = b.col_index("g").unwrap_or(0);
        let order = b.order_by(|row| row.get(g).as_str().unwrap_or("").to_string());
        b.take_rows(order);
        assert_eq!(column(&b, "n"), ["1", "3", "2", "4"]);
        let order =
            b.order_by(|row| std::cmp::Reverse(row.get(g).as_str().unwrap_or("").to_string()));
        b.take_rows(order);
        assert_eq!(column(&b, "n"), ["2", "4", "1", "3"]);
    }

    #[test]
    fn select_columns_reorders_and_repeats() {
        let mut b = batch(&[
            json!({"a": 1, "b": "x", "c": true}),
            json!({"a": 2, "b": "y", "c": false}),
        ]);
        b.select_columns(&[2, 0, 2]);
        let names: Vec<&str> = b.schema().names().collect();
        assert_eq!(names, ["c", "a", "c"]);
        assert_eq!(b.nrows(), 2);
        let flat: Vec<String> = b
            .rows()
            .flat_map(|row| {
                row.iter()
                    .map(|d| d.text(&mut String::new()).to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(flat, ["true", "1", "true", "false", "2", "false"]);
    }

    #[test]
    fn a_batch_without_columns_has_no_rows() {
        let b = RecordBatch::empty();
        assert_eq!(b.rows().count(), 0);
    }

    #[test]
    fn take_rows_ignores_an_order_made_for_a_batch_of_another_height() {
        let long = batch(&[json!({"a": 1}), json!({"a": 2}), json!({"a": 3})]);
        let mut short = batch(&[json!({"a": 9}), json!({"a": 8})]);
        let order = long.order_by(|row| row.get(0).as_f64().map(|n| n as i64));
        short.take_rows(order);
        assert_eq!(column(&short, "a"), ["9", "8"]);
    }
}
