use crate::typed::{FileMode, UnixTime};
use crate::{ColType, Column, Datum};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Cell {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str { len: u32, off: u64 },
    Json { len: u32, off: u64 },
}

const _: () = assert!(size_of::<Cell>() == 16);

impl Cell {
    pub(crate) fn datum<'a>(self, text: &'a str, columns: &[Column], col: usize) -> Datum<'a> {
        match self {
            Cell::Null => Datum::Null,
            Cell::Bool(b) => Datum::Bool(b),
            Cell::Int(n) => Datum::Int(n),
            Cell::UInt(n) => match columns[col].ty() {
                ColType::Timestamp => Datum::Time(UnixTime(n)),
                ColType::Mode => Datum::Mode(FileMode(n as u32)),
                _ => Datum::UInt(n),
            },
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
