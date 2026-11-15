use std::borrow::Cow;
use std::sync::Arc;

use crate::{CellVal, StructuredRecord};

#[derive(Debug, Clone)]
pub struct RecordBatch {
    pub columns: Arc<[Cow<'static, str>]>,
    pub cells: Vec<CellVal>,
}

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

    pub fn row(&self, idx: usize) -> &[CellVal] {
        let ncols = self.columns.len();
        &self.cells[idx * ncols..(idx + 1) * ncols]
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

    pub fn to_maps(&self) -> Vec<StructuredRecord> {
        let ncols = self.columns.len();
        if ncols == 0 {
            return Vec::new();
        }
        let nrows = self.nrows();
        let mut out = Vec::with_capacity(nrows);
        for r in 0..nrows {
            let row = self.row(r);
            let mut map = StructuredRecord::new();
            for (c, col) in self.columns.iter().enumerate() {
                if !matches!(row[c], CellVal::Null) {
                    map.insert(col.to_string(), row[c].to_json());
                }
            }
            out.push(map);
        }
        out
    }
}
