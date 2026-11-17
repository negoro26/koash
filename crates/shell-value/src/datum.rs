use std::fmt::{Display, Write as _};
use std::io::Write;

use crate::typed::{FileMode, ShortText, UnixTime};

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Datum<'a> {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(&'a str),
    Json(&'a str),
    Time(UnixTime),
    Mode(FileMode),
}

impl<'a> Datum<'a> {
    pub fn is_null(&self) -> bool {
        matches!(self, Datum::Null)
    }

    pub fn as_str(&self) -> Option<&'a str> {
        match *self {
            Datum::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn short_text(&self) -> Option<ShortText> {
        match *self {
            Datum::Time(time) => Some(time.text()),
            Datum::Mode(mode) => Some(mode.text()),
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

    pub fn text<'s>(&self, scratch: &'s mut String) -> &'s str
    where
        'a: 's,
    {
        match *self {
            Datum::Null => "",
            Datum::Bool(true) => "true",
            Datum::Bool(false) => "false",
            Datum::Str(s) | Datum::Json(s) => s,
            Datum::Int(n) => format_into(scratch, &n),
            Datum::UInt(n) => format_into(scratch, &n),
            Datum::Float(n) => format_into(scratch, &n),
            Datum::Time(time) => set_text(scratch, time.text().as_str()),
            Datum::Mode(mode) => set_text(scratch, mode.text().as_str()),
        }
    }

    pub fn text_len(&self, scratch: &mut String) -> usize {
        match *self {
            Datum::Null => 0,
            Datum::Bool(true) => 4,
            Datum::Bool(false) => 5,
            Datum::Str(s) | Datum::Json(s) => s.len(),
            Datum::UInt(u) => uint_decimal_len(u),
            Datum::Int(i) if i >= 0 => uint_decimal_len(i as u64),
            Datum::Int(i) => 1 + uint_decimal_len(i.wrapping_neg() as u64),
            Datum::Float(f) => format_into(scratch, &f).len(),
            Datum::Time(time) => time.text().len(),
            Datum::Mode(mode) => mode.text().len(),
        }
    }

    pub fn write_text<W: Write>(&self, w: &mut W, scratch: &mut String) -> usize {
        match *self {
            Datum::UInt(u) => write_uint_stack(w, u),
            Datum::Int(i) if i >= 0 => write_uint_stack(w, i as u64),
            Datum::Int(i) => {
                let _ = w.write_all(b"-");
                1 + write_uint_stack(w, i.wrapping_neg() as u64)
            }
            Datum::Time(_) | Datum::Mode(_) => {
                let text = self.short_text().unwrap_or_default();
                let _ = w.write_all(text.as_str().as_bytes());
                text.len()
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

fn set_text<'s>(scratch: &'s mut String, text: &str) -> &'s str {
    scratch.clear();
    scratch.push_str(text);
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
