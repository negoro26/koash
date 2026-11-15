use crate::Datum;

/// One table cell. Text lives in the batch's arena, so a cell is plain data: `Copy`, no `Drop`,
/// and meaningless without the batch that made it.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str {
        len: u32,
        off: u64,
    },
    /// The compact text of a JSON array or object.
    Json {
        len: u32,
        off: u64,
    },
}

const _: () = assert!(size_of::<Cell>() == 16);

impl Cell {
    pub(crate) fn datum(self, text: &str) -> Datum<'_> {
        match self {
            Cell::Null => Datum::Null,
            Cell::Bool(b) => Datum::Bool(b),
            Cell::Int(n) => Datum::Int(n),
            Cell::UInt(n) => Datum::UInt(n),
            Cell::Float(n) => Datum::Float(n),
            Cell::Str { len, off } => Datum::Str(span(text, len, off)),
            Cell::Json { len, off } => Datum::Json(span(text, len, off)),
        }
    }
}

pub(crate) fn span(text: &str, len: u32, off: u64) -> &str {
    let start = off as usize;
    text.get(start..start + len as usize).unwrap_or("")
}
