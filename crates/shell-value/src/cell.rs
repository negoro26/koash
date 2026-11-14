use std::io::Write;
use std::sync::Arc;

use crate::json::format_cell;

#[derive(Debug, Clone)]
pub enum CellVal {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    SharedStr(Arc<str>),
    Str(String),
    Hash([u8; 3]),
    Json(serde_json::Value),
}

impl CellVal {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            CellVal::SharedStr(s) => Some(s),
            CellVal::Str(s) => Some(s),
            CellVal::Hash(h) => std::str::from_utf8(h).ok(),
            CellVal::Json(serde_json::Value::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            CellVal::Int(n) => Some(*n as f64),
            CellVal::UInt(n) => Some(*n as f64),
            CellVal::Float(n) => Some(*n),
            CellVal::Json(v) => v.as_f64(),
            _ => None,
        }
    }

    pub fn from_json(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => CellVal::Null,
            serde_json::Value::Bool(b) => CellVal::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    CellVal::UInt(u)
                } else if let Some(i) = n.as_i64() {
                    CellVal::Int(i)
                } else {
                    CellVal::Float(n.as_f64().unwrap_or(0.0))
                }
            }
            serde_json::Value::String(s) => CellVal::Str(s),
            other => CellVal::Json(other),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            CellVal::Null => serde_json::Value::Null,
            CellVal::Bool(b) => serde_json::Value::Bool(*b),
            CellVal::Int(n) => serde_json::Value::from(*n),
            CellVal::UInt(n) => serde_json::Value::from(*n),
            CellVal::Float(n) => serde_json::Value::from(*n),
            CellVal::SharedStr(s) => serde_json::Value::String(s.to_string()),
            CellVal::Str(s) => serde_json::Value::String(s.clone()),
            CellVal::Hash(h) => serde_json::Value::String(String::from_utf8_lossy(h).into_owned()),
            CellVal::Json(v) => v.clone(),
        }
    }

    pub fn display_len(&self, scratch: &mut String) -> usize {
        match self {
            CellVal::Null => 0,
            CellVal::Bool(true) => 4,
            CellVal::Bool(false) => 5,
            CellVal::Hash(_) => 3,
            CellVal::SharedStr(s) => s.len(),
            CellVal::Str(s) => s.len(),
            CellVal::UInt(u) => uint_decimal_len(*u),
            CellVal::Int(i) => {
                if *i >= 0 {
                    uint_decimal_len(*i as u64)
                } else {
                    1 + uint_decimal_len(i.wrapping_neg() as u64)
                }
            }
            CellVal::Float(f) => {
                scratch.clear();
                let _ = std::fmt::Write::write_fmt(scratch, format_args!("{f}"));
                scratch.len()
            }
            CellVal::Json(v) => {
                scratch.clear();
                let _ = std::fmt::Write::write_fmt(scratch, format_args!("{v}"));
                scratch.len()
            }
        }
    }

    pub fn write_to<W: Write>(&self, w: &mut W, scratch: &mut String) -> usize {
        match self {
            CellVal::Null => 0,
            CellVal::Bool(true) => {
                let _ = w.write_all(b"true");
                4
            }
            CellVal::Bool(false) => {
                let _ = w.write_all(b"false");
                5
            }
            CellVal::Hash(h) => {
                let _ = w.write_all(h);
                3
            }
            CellVal::SharedStr(s) => {
                let _ = w.write_all(s.as_bytes());
                s.len()
            }
            CellVal::Str(s) => {
                let _ = w.write_all(s.as_bytes());
                s.len()
            }
            CellVal::UInt(u) => write_uint_stack(w, *u),
            CellVal::Int(i) => {
                if *i >= 0 {
                    write_uint_stack(w, *i as u64)
                } else {
                    let _ = w.write_all(b"-");
                    1 + write_uint_stack(w, i.wrapping_neg() as u64)
                }
            }
            CellVal::Float(f) => {
                scratch.clear();
                let _ = std::fmt::Write::write_fmt(scratch, format_args!("{f}"));
                let _ = w.write_all(scratch.as_bytes());
                scratch.len()
            }
            CellVal::Json(v) => {
                scratch.clear();
                let _ = std::fmt::Write::write_fmt(scratch, format_args!("{v}"));
                let _ = w.write_all(scratch.as_bytes());
                scratch.len()
            }
        }
    }

    pub fn format_to_string(&self) -> String {
        match self {
            CellVal::Null => String::new(),
            CellVal::Bool(b) => b.to_string(),
            CellVal::Int(n) => n.to_string(),
            CellVal::UInt(n) => n.to_string(),
            CellVal::Float(n) => n.to_string(),
            CellVal::SharedStr(s) => s.to_string(),
            CellVal::Str(s) => s.clone(),
            CellVal::Hash(h) => String::from_utf8_lossy(h).into_owned(),
            CellVal::Json(v) => format_cell(v),
        }
    }
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
