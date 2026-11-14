use std::borrow::Cow;
use std::sync::Arc;

use crate::{CellVal, Datum, StructuredRecord, json};

#[derive(Debug, Clone)]
pub struct RecordBatch {
    pub columns: Arc<[Cow<'static, str>]>,
    pub cells: Vec<CellVal>,
}

/// One row of a batch, read through [`Datum`].
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    cells: &'a [CellVal],
}

impl<'a> Row<'a> {
    pub fn get(&self, col: usize) -> Datum<'a> {
        self.cells[col].datum()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = Datum<'a>> + use<'a> {
        self.cells.iter().map(CellVal::datum)
    }
}

/// A permutation of a batch's rows, made by [`RecordBatch::order_by`].
#[derive(Debug)]
pub struct RowOrder(Vec<usize>);

impl RecordBatch {
    pub fn new(columns: Arc<[Cow<'static, str>]>) -> Self {
        Self {
            columns,
            cells: Vec::new(),
        }
    }

    pub fn with_capacity(columns: Arc<[Cow<'static, str>]>, row_cap: usize) -> Self {
        let ncols = columns.len();
        Self {
            columns,
            cells: Vec::with_capacity(row_cap * ncols),
        }
    }

    pub fn empty() -> Self {
        Self {
            columns: Arc::from([]),
            cells: Vec::new(),
        }
    }

    pub fn ncols(&self) -> usize {
        self.columns.len()
    }

    pub fn nrows(&self) -> usize {
        self.cells
            .len()
            .checked_div(self.columns.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.nrows() == 0
    }

    pub fn row(&self, idx: usize) -> Row<'_> {
        let ncols = self.columns.len();
        Row {
            cells: &self.cells[idx * ncols..(idx + 1) * ncols],
        }
    }

    pub fn col_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.as_ref() == name)
    }

    pub fn truncate_rows(&mut self, n: usize) {
        let keep = n.min(self.nrows()) * self.columns.len();
        self.cells.truncate(keep);
    }

    pub fn tail_rows(&mut self, n: usize) {
        let nrows = self.nrows();
        let drop_rows = nrows.saturating_sub(n);
        if drop_rows > 0 {
            self.cells.drain(..drop_rows * self.columns.len());
        }
    }

    pub fn rows(&self) -> impl ExactSizeIterator<Item = Row<'_>> {
        self.cells
            .chunks_exact(self.columns.len().max(1))
            .map(|cells| Row { cells })
    }

    /// Keeps the rows `keep` accepts, in order, without copying any cell.
    pub fn retain_rows(&mut self, mut keep: impl FnMut(Row<'_>) -> bool) {
        let ncols = self.columns.len();
        let mut write = 0;
        for read in 0..self.nrows() {
            let cells = &self.cells[read * ncols..(read + 1) * ncols];
            if !keep(Row { cells }) {
                continue;
            }
            if write != read {
                for c in 0..ncols {
                    self.cells.swap(write * ncols + c, read * ncols + c);
                }
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

    /// Reorders the rows by moving their cells.
    pub fn take_rows(&mut self, order: RowOrder) {
        let ncols = self.columns.len();
        assert_eq!(
            order.0.len(),
            self.nrows(),
            "the order comes from another batch"
        );
        let mut src = std::mem::take(&mut self.cells);
        let mut cells = Vec::with_capacity(src.len());
        for idx in order.0 {
            for cell in &mut src[idx * ncols..(idx + 1) * ncols] {
                cells.push(std::mem::replace(cell, CellVal::Null));
            }
        }
        self.cells = cells;
    }

    /// Keeps the listed columns in the listed order. A column listed twice appears twice.
    pub fn select_columns(&mut self, cols: &[usize]) {
        let ncols = self.columns.len();
        let nrows = self.nrows();
        let first_use: Vec<usize> = cols
            .iter()
            .enumerate()
            .map(|(at, col)| cols.iter().position(|c| c == col).unwrap_or(at))
            .collect();
        let mut src = std::mem::take(&mut self.cells);
        let mut cells: Vec<CellVal> = Vec::with_capacity(nrows * cols.len());
        for row in src.chunks_exact_mut(ncols.max(1)).take(nrows) {
            let base = cells.len();
            for (at, &col) in cols.iter().enumerate() {
                let cell = if first_use[at] == at {
                    std::mem::replace(&mut row[col], CellVal::Null)
                } else {
                    cells[base + first_use[at]].clone()
                };
                cells.push(cell);
            }
        }
        self.columns = cols.iter().map(|&col| self.columns[col].clone()).collect();
        self.cells = cells;
    }

    pub fn from_maps(records: Vec<StructuredRecord>) -> Self {
        if records.is_empty() {
            return Self::empty();
        }
        let mut columns: Vec<Cow<'static, str>> = Vec::new();
        for rec in &records {
            for k in rec.keys() {
                if !columns.iter().any(|c| c.as_ref() == k.as_str()) {
                    columns.push(Cow::Owned(k.clone()));
                }
            }
        }
        let ncols = columns.len();
        let mut cells = Vec::with_capacity(records.len() * ncols);
        for mut rec in records {
            for col in &columns {
                let cell = match rec.remove(col.as_ref()) {
                    Some(v) => CellVal::from_json(v),
                    None => CellVal::Null,
                };
                cells.push(cell);
            }
        }
        Self {
            columns: Arc::from(columns),
            cells,
        }
    }

    /// The first `rows` rows of `batches` as one batch.
    ///
    /// The columns are the names that have a non-null cell in those rows, in the order a walk over
    /// the rows meets them, each row visited in name order. Cells follow the JSON rule: a name
    /// repeated in a batch keeps its last non-null cell. Rows with no non-null cell are lost.
    pub fn merged(batches: &[RecordBatch], rows: usize) -> Self {
        let mut names: Vec<Cow<'static, str>> = Vec::new();
        let mut left = rows;
        for batch in batches {
            let order = json::name_order(batch);
            for row in batch.rows().take(left) {
                for col in json::visible_columns(&batch.columns, &order, row) {
                    let name = &batch.columns[col];
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
            left = left.saturating_sub(batch.nrows());
        }

        let mut cells = Vec::new();
        let mut left = rows;
        for batch in batches {
            let order = json::name_order(batch);
            let slot: Vec<Option<usize>> = batch
                .columns
                .iter()
                .map(|name| names.iter().position(|n| n == name))
                .collect();
            for row in batch.rows().take(left) {
                let start = cells.len();
                cells.resize(start + names.len(), CellVal::Null);
                for col in json::visible_columns(&batch.columns, &order, row) {
                    if let Some(at) = slot[col] {
                        cells[start + at] = CellVal::from(row.get(col));
                    }
                }
            }
            left = left.saturating_sub(batch.nrows());
        }
        Self {
            columns: names.into(),
            cells,
        }
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
        RecordBatch::from_maps(maps)
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
        let names: Vec<&str> = b.columns.iter().map(|c| c.as_ref()).collect();
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
}
