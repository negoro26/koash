use std::marker::PhantomData;

use crate::cell::{Cell, span};
use crate::typed::{FileMode, OrBlank, UnixTime};
use crate::{ColType, Column, Datum, RecordBatch, Schema, StructuredDiagnostic};

pub trait CellValue {
    const TYPE: ColType = ColType::Any;

    fn put(&self, row: &mut RowSink<'_>);
}

impl CellValue for &str {
    fn put(&self, row: &mut RowSink<'_>) {
        row.put_str(self);
    }
}

impl CellValue for u64 {
    fn put(&self, row: &mut RowSink<'_>) {
        row.put(Datum::UInt(*self));
    }
}

impl CellValue for Datum<'_> {
    fn put(&self, row: &mut RowSink<'_>) {
        row.put(*self);
    }
}

impl CellValue for bool {
    fn put(&self, row: &mut RowSink<'_>) {
        row.put(Datum::Bool(*self));
    }
}

impl CellValue for UnixTime {
    const TYPE: ColType = ColType::Timestamp;

    fn put(&self, row: &mut RowSink<'_>) {
        row.put(Datum::Time(*self));
    }
}

impl CellValue for FileMode {
    const TYPE: ColType = ColType::Mode;

    fn put(&self, row: &mut RowSink<'_>) {
        row.put(Datum::Mode(*self));
    }
}

impl<T: CellValue> CellValue for OrBlank<T> {
    const TYPE: ColType = T::TYPE;

    fn put(&self, row: &mut RowSink<'_>) {
        match &self.0 {
            Some(value) => value.put(row),
            None => row.put_str(""),
        }
    }
}

pub trait Record {
    type Table: Record + 'static;
    const COLUMNS: &'static [Column];
    fn write(&self, row: &mut RowSink<'_>);
}

#[macro_export]
macro_rules! record {
    ($vis:vis struct $name:ident<$life:lifetime> { $($(#[$mark:ident])? $field:ident: $ty:ty),+ $(,)? }) => {
        $vis struct $name<$life> {
            $(pub $field: $ty),+
        }

        impl<$life> $crate::Record for $name<$life> {
            type Table = $name<'static>;
            const COLUMNS: &'static [$crate::Column] =
                &[$($crate::Column::typed(stringify!($field), <$ty as $crate::CellValue>::TYPE)$(.$mark())?),+];

            fn write(&self, row: &mut $crate::RowSink<'_>) {
                $($crate::CellValue::put(&self.$field, row);)+
            }
        }
    };
}

struct Builder {
    schema: Schema,
    cells: Vec<Cell>,
    text: String,
    too_large: bool,
    typed: bool,
}

impl Builder {
    fn new(schema: Schema, row_cap: usize) -> Self {
        let cells = Vec::with_capacity(row_cap * schema.len());
        let typed = schema.columns().iter().any(|c| c.ty() != ColType::Any);
        Self {
            schema,
            cells,
            text: String::new(),
            too_large: false,
            typed,
        }
    }

    fn push_row(&mut self, fill: impl FnOnce(&mut RowSink<'_>)) {
        let ncols = self.schema.len();
        let start = self.cells.len();
        let mut sink = RowSink {
            builder: self,
            start,
            col: 0,
        };
        fill(&mut sink);
        self.cells.resize(start + ncols, Cell::Null);
    }

    fn append(&mut self, s: &str) -> Option<(u32, u64)> {
        let Ok(len) = u32::try_from(s.len()) else {
            self.too_large = true;
            return None;
        };
        let off = self.text.len() as u64;
        self.text.push_str(s);
        Some((len, off))
    }

    fn into_batch(self) -> RecordBatch {
        RecordBatch::from_parts(self.schema, self.cells, self.text)
    }

    fn finish(self) -> Result<RecordBatch, StructuredDiagnostic> {
        if self.too_large {
            return Err(StructuredDiagnostic::new(
                "E_TOO_LARGE",
                "a table cell is 4 GiB or longer",
                None,
                None,
            ));
        }
        Ok(self.into_batch())
    }

    fn finish_or_empty(self) -> Result<RecordBatch, StructuredDiagnostic> {
        let batch = self.finish()?;
        Ok(if batch.is_empty() {
            RecordBatch::empty()
        } else {
            batch
        })
    }
}

pub struct RowSink<'b> {
    builder: &'b mut Builder,
    start: usize,
    col: usize,
}

impl RowSink<'_> {
    pub fn put(&mut self, datum: Datum<'_>) {
        match datum {
            Datum::Null => self.push(Cell::Null),
            Datum::Bool(b) => self.push(Cell::Bool(b)),
            Datum::Int(n) => self.push(Cell::Int(n)),
            Datum::UInt(n) => match self.ty() {
                ColType::Any => self.push(Cell::UInt(n)),
                _ => self.put_str(&n.to_string()),
            },
            Datum::Float(n) => self.push(Cell::Float(n)),
            Datum::Str(s) => self.put_str(s),
            Datum::Time(time) => match self.ty() {
                ColType::Timestamp => self.push(Cell::UInt(time.0)),
                _ => self.put_str(time.text().as_str()),
            },
            Datum::Mode(mode) => match self.ty() {
                ColType::Mode => self.push(Cell::UInt(u64::from(mode.0))),
                _ => self.put_str(mode.text().as_str()),
            },
            Datum::Json(s) => {
                let cell = match self.builder.append(s) {
                    Some((len, off)) => Cell::Json { len, off },
                    None => Cell::Null,
                };
                self.push(cell);
            }
        }
    }

    pub fn put_str(&mut self, s: &str) {
        let above = self.above();
        let builder = &mut *self.builder;
        let cell = match above {
            Some(Cell::Str { len, off }) if span(&builder.text, len, off) == s => {
                Cell::Str { len, off }
            }
            _ => match builder.append(s) {
                Some((len, off)) => Cell::Str { len, off },
                None => Cell::Null,
            },
        };
        self.push(cell);
    }

    fn ty(&self) -> ColType {
        if !self.builder.typed {
            return ColType::Any;
        }
        let columns = self.builder.schema.columns();
        columns.get(self.col).map_or(ColType::Any, Column::ty)
    }

    fn above(&self) -> Option<Cell> {
        let ncols = self.builder.schema.len();
        if self.col >= ncols {
            return None;
        }
        let at = (self.start + self.col).checked_sub(ncols)?;
        self.builder.cells.get(at).copied()
    }

    fn push(&mut self, cell: Cell) {
        if self.col < self.builder.schema.len() {
            self.builder.cells.push(cell);
        }
        self.col += 1;
    }
}

pub struct Rows<T> {
    builder: Builder,
    table: PhantomData<fn() -> T>,
}

impl<T: Record> Rows<T> {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(rows: usize) -> Self {
        Self {
            builder: Builder::new(Schema::of(T::COLUMNS), rows),
            table: PhantomData,
        }
    }

    pub fn push<R: Record<Table = T>>(&mut self, row: &R) {
        self.builder.push_row(|sink| row.write(sink));
    }

    pub fn finish(self) -> Result<RecordBatch, StructuredDiagnostic> {
        self.builder.finish()
    }

    pub fn finish_or_empty(self) -> Result<RecordBatch, StructuredDiagnostic> {
        self.builder.finish_or_empty()
    }
}

impl<T: Record> Default for Rows<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct DynRows {
    builder: Builder,
}

impl DynRows {
    pub fn new(schema: Schema) -> Self {
        Self {
            builder: Builder::new(schema, 0),
        }
    }

    pub fn push_with(&mut self, fill: impl FnOnce(&mut RowSink<'_>)) {
        self.builder.push_row(fill);
    }

    pub fn finish(self) -> Result<RecordBatch, StructuredDiagnostic> {
        self.builder.finish()
    }

    pub(crate) fn into_batch(self) -> RecordBatch {
        self.builder.into_batch()
    }
}
