use std::fmt::{Display, Write as _};
use std::io::Write;

/// One cell as callers read it. Storage may change; this view does not.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Datum<'a> {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(&'a str),
    /// A JSON array or object.
    Json(&'a serde_json::Value),
}

impl<'a> Datum<'a> {
    pub fn is_null(&self) -> bool {
        matches!(self, Datum::Null)
    }

    /// The text of a string cell. Numbers, booleans and nested JSON are not strings.
    pub fn as_str(&self) -> Option<&'a str> {
        match *self {
            Datum::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Datum::Int(n) => Some(n as f64),
            Datum::UInt(n) => Some(n as f64),
            Datum::Float(n) => Some(n),
            _ => None,
        }
    }

    /// What the table and the compact format print for this cell. Null is empty.
    pub fn text<'s>(&self, scratch: &'s mut String) -> &'s str
    where
        'a: 's,
    {
        match *self {
            Datum::Null => "",
            Datum::Bool(true) => "true",
            Datum::Bool(false) => "false",
            Datum::Str(s) => s,
            Datum::Int(n) => format_into(scratch, &n),
            Datum::UInt(n) => format_into(scratch, &n),
            Datum::Float(n) => format_into(scratch, &n),
            Datum::Json(v) => format_into(scratch, v),
        }
    }

    /// Byte length of [`Datum::text`], without formatting integers.
    pub fn text_len(&self, scratch: &mut String) -> usize {
        match *self {
            Datum::Null => 0,
            Datum::Bool(true) => 4,
            Datum::Bool(false) => 5,
            Datum::Str(s) => s.len(),
            Datum::UInt(u) => uint_decimal_len(u),
            Datum::Int(i) if i >= 0 => uint_decimal_len(i as u64),
            Datum::Int(i) => 1 + uint_decimal_len(i.wrapping_neg() as u64),
            Datum::Float(f) => format_into(scratch, &f).len(),
            Datum::Json(v) => format_into(scratch, v).len(),
        }
    }

    /// Writes [`Datum::text`] and returns its byte length.
    pub fn write_text<W: Write>(&self, w: &mut W, scratch: &mut String) -> usize {
        match *self {
            Datum::UInt(u) => write_uint_stack(w, u),
            Datum::Int(i) if i >= 0 => write_uint_stack(w, i as u64),
            Datum::Int(i) => {
                let _ = w.write_all(b"-");
                1 + write_uint_stack(w, i.wrapping_neg() as u64)
            }
            other => {
                let text = other.text(scratch);
                let _ = w.write_all(text.as_bytes());
                text.len()
            }
        }
    }
}

fn format_into<'s>(scratch: &'s mut String, value: &impl Display) -> &'s str {
    scratch.clear();
    let _ = write!(scratch, "{value}");
    scratch
}

fn uint_decimal_len(mut n: u64) -> usize {
    let mut len = 1usize;
    while n >= 10 {
        n /= 10;
        len += 1;
    }
    len
}

fn write_uint_stack<W: Write>(w: &mut W, mut n: u64) -> usize {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let slice = &buf[i..];
    let _ = w.write_all(slice);
    slice.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(d: Datum<'_>) -> String {
        d.text(&mut String::new()).to_string()
    }

    #[test]
    fn text_is_what_the_table_prints() {
        let nested = serde_json::json!([1, {"a": null}]);
        assert_eq!(text(Datum::Null), "");
        assert_eq!(text(Datum::Bool(true)), "true");
        assert_eq!(text(Datum::Int(-7)), "-7");
        assert_eq!(text(Datum::UInt(u64::MAX)), "18446744073709551615");
        assert_eq!(text(Datum::Float(2.0)), "2");
        assert_eq!(text(Datum::Float(1e21)), "1000000000000000000000");
        assert_eq!(text(Datum::Str("é")), "é");
        assert_eq!(text(Datum::Json(&nested)), "[1,{\"a\":null}]");
    }

    #[test]
    fn text_len_and_write_text_agree_with_text() {
        let object = serde_json::json!({});
        let cells = [
            Datum::Null,
            Datum::Bool(false),
            Datum::Int(i64::MIN),
            Datum::Int(0),
            Datum::UInt(10),
            Datum::Float(-0.5),
            Datum::Str("héllo"),
            Datum::Json(&object),
        ];
        let mut scratch = String::new();
        for cell in cells {
            let expected = text(cell);
            let mut written = Vec::new();
            assert_eq!(cell.text_len(&mut scratch), expected.len());
            assert_eq!(cell.write_text(&mut written, &mut scratch), expected.len());
            assert_eq!(written, expected.as_bytes());
        }
    }

    #[test]
    fn only_strings_are_strings_and_only_numbers_are_numbers() {
        let array = serde_json::json!([1]);
        assert_eq!(Datum::Str("1").as_f64(), None);
        assert_eq!(Datum::Int(1).as_str(), None);
        assert_eq!(Datum::Json(&array).as_f64(), None);
        assert_eq!(Datum::Bool(true).as_f64(), None);
        assert_eq!(Datum::UInt(3).as_f64(), Some(3.0));
        assert_eq!(Datum::Float(0.5).as_f64(), Some(0.5));
    }
}
