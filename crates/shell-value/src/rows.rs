use std::marker::PhantomData;

use crate::cell::{Cell, span};
use crate::typed::{FileMode, OrBlank, UnixTime};
use crate::{ColType, Column, Datum, RecordBatch, Schema, StructuredDiagnostic};

/// A value that knows how to write itself as one cell.
pub trait CellValue {
    /// How a column of these values reads. [`RowSink::put`] stores the value so that it does.
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

/// A row shape fixed at compile time. Only [`record!`] implements it, so the columns and the
/// order cells are written in come from one list.
pub trait Record {
    /// The same row with every borrow `'static`, so a [`Rows`] can outlive the data each row borrows.
    type Table: Record + 'static;
    const COLUMNS: &'static [Column];
    fn write(&self, row: &mut RowSink<'_>);
}

/// Declares a [`Record`]: a struct whose field order is the column order.
///
/// ```ignore
/// record! { pub struct CountRow<'a> { file: &'a str, count: u64 } }
/// ```
#[macro_export]
macro_rules! record {
    ($vis:vis struct $name:ident<$life:lifetime> { $($field:ident: $ty:ty),+ $(,)? }) => {
        $vis struct $name<$life> {
            $(pub $field: $ty),+
        }

        impl<$life> $crate::Record for $name<$life> {
            type Table = $name<'static>;
            const COLUMNS: &'static [$crate::Column] =
                &[$($crate::Column::typed(stringify!($field), <$ty as $crate::CellValue>::TYPE)),+];

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

/// Where one row's cells go. A row shorter than the schema is padded with nulls and cells past
/// the end are dropped, so a batch is always rectangular.
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

    /// A string cell. When it equals the cell above it, both share one span of the arena.
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

    /// A string cell from bytes that may not be UTF-8. Invalid sequences show as U+FFFD.
    pub fn put_lossy(&mut self, bytes: &[u8]) {
        self.put_str(&String::from_utf8_lossy(bytes));
    }

    /// The type of the column this cell goes into. A `UInt` is only a number in an untyped column,
    /// because a typed column reads every `UInt` as a time or a mode.
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

/// Writes rows of one [`Record`] type into a batch.
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

    /// The batch, which keeps its schema when it has no rows.
    pub fn finish(self) -> Result<RecordBatch, StructuredDiagnostic> {
        self.builder.finish()
    }

    /// Like [`Rows::finish`], except that a batch with no rows has no columns either. That is how
    /// every builtin that built its table from maps reported an empty result, and what `grep`
    /// downstream of it relies on.
    pub fn finish_or_empty(self) -> Result<RecordBatch, StructuredDiagnostic> {
        self.builder.finish_or_empty()
    }
}

impl<T: Record> Default for Rows<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Writes rows into a batch whose columns are only known at run time.
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

    /// The batch, which keeps its schema when it has no rows.
    pub fn finish(self) -> Result<RecordBatch, StructuredDiagnostic> {
        self.builder.finish()
    }

    pub(crate) fn into_batch(self) -> RecordBatch {
        self.builder.into_batch()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    record! { struct Pair<'a> { name: &'a str, n: u64 } }

    fn texts(batch: &RecordBatch) -> Vec<Vec<String>> {
        let mut scratch = String::new();
        batch
            .rows()
            .map(|row| {
                row.iter()
                    .map(|d| d.text(&mut scratch).to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_record_writes_its_fields_in_declaration_order() {
        let mut rows = Rows::<Pair<'static>>::new();
        let owned = String::from("a");
        rows.push(&Pair { name: &owned, n: 1 });
        rows.push(&Pair { name: "b", n: 2 });
        let batch = rows.finish().unwrap();
        assert_eq!(
            batch.schema().names().collect::<Vec<_>>(),
            Pair::COLUMNS.iter().map(Column::name).collect::<Vec<_>>()
        );
        assert_eq!(texts(&batch), [["a", "1"], ["b", "2"]]);
    }

    #[test]
    fn a_short_row_is_padded_and_extra_cells_are_dropped() {
        let mut rows = DynRows::new(Schema::named(["a", "b"]));
        rows.push_with(|out| out.put(Datum::Int(1)));
        rows.push_with(|out| {
            for n in 0..4 {
                out.put(Datum::Int(n));
            }
        });
        let batch = rows.finish().unwrap();
        assert_eq!(texts(&batch), [["1", ""], ["0", "1"]]);
    }

    #[test]
    fn a_string_equal_to_the_cell_above_shares_its_text() {
        let mut rows = DynRows::new(Schema::named(["label", "text"]));
        for line in ["x", "y", "x"] {
            rows.push_with(|out| {
                out.put_str("same.txt");
                out.put_str(line);
            });
        }
        let batch = rows.finish().unwrap();
        assert_eq!(batch.text_bytes(), "same.txt".len() + 3);
        assert_eq!(
            texts(&batch),
            [["same.txt", "x"], ["same.txt", "y"], ["same.txt", "x"]]
        );
    }

    #[test]
    fn lossy_text_replaces_invalid_bytes() {
        let mut rows = DynRows::new(Schema::named(["t"]));
        rows.push_with(|out| out.put_lossy(b"a\xffb"));
        assert_eq!(texts(&rows.finish().unwrap()), [["a\u{fffd}b"]]);
    }

    #[test]
    fn an_empty_batch_keeps_its_schema() {
        let batch = Rows::<Pair<'static>>::new().finish().unwrap();
        assert_eq!((batch.nrows(), batch.ncols()), (0, 2));
    }
}
