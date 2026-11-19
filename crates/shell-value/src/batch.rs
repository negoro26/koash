use std::ops::Range;

use crate::cell::Cell;
use crate::rows::DynRows;
use crate::{Column, Datum, Schema, StructuredDiagnostic, StructuredRecord, json};

#[derive(Debug, Clone)]
pub struct RecordBatch {
    schema: Schema,
    cells: Vec<Cell>,
    text: String,
}

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

    pub fn write_line(&self, cols: Range<usize>, out: &mut Vec<u8>, scratch: &mut String) {
        if cols.len() == 1 {
            self.get(cols.start).write_text(out, scratch);
            return;
        }
        for col in cols.clone() {
            if col != cols.start {
                out.push(b'\t');
            }
            write_tsv_field(out, self.get(col).text(scratch));
        }
    }
}

#[derive(Debug)]
pub struct RowOrder(Vec<usize>);

impl RowOrder {
    pub fn reverse(mut self) -> Self {
        self.0.reverse();
        self
    }
}

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

    pub fn order_by<'s, K: Ord>(&'s self, mut key: impl FnMut(Row<'s>) -> K) -> RowOrder {
        self.order_by_index(|_, row| key(row))
    }

    pub fn order_by_index<'s, K: Ord>(
        &'s self,
        mut key: impl FnMut(usize, Row<'s>) -> K,
    ) -> RowOrder {
        let mut keyed: Vec<(K, usize)> = self
            .rows()
            .enumerate()
            .map(|(idx, row)| (key(idx, row), idx))
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        RowOrder(keyed.into_iter().map(|(_, idx)| idx).collect())
    }

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

    pub fn write_lines(&self, out: &mut Vec<u8>) {
        let cols = match self.schema.text_column() {
            Some(col) => col..col + 1,
            None => 0..self.ncols(),
        };
        let mut scratch = String::new();
        for row in self.rows() {
            row.write_line(cols.clone(), out, &mut scratch);
            out.push(b'\n');
        }
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
}

fn write_tsv_field(out: &mut Vec<u8>, text: &str) {
    for &byte in text.as_bytes() {
        match byte {
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            _ => out.push(byte),
        }
    }
}
