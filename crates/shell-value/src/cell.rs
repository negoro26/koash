use std::sync::Arc;

use crate::Datum;

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
    pub fn datum(&self) -> Datum<'_> {
        match self {
            CellVal::Null => Datum::Null,
            CellVal::Bool(b) => Datum::Bool(*b),
            CellVal::Int(n) => Datum::Int(*n),
            CellVal::UInt(n) => Datum::UInt(*n),
            CellVal::Float(n) => Datum::Float(*n),
            CellVal::SharedStr(s) => Datum::Str(s),
            CellVal::Str(s) => Datum::Str(s),
            CellVal::Hash(h) => Datum::Str(std::str::from_utf8(h).unwrap_or("")),
            CellVal::Json(v) => Datum::Json(v),
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
}

impl From<Datum<'_>> for CellVal {
    /// A three-byte string becomes `Hash`, which stores it inline and reads back as the same string.
    fn from(d: Datum<'_>) -> Self {
        match d {
            Datum::Null => CellVal::Null,
            Datum::Bool(b) => CellVal::Bool(b),
            Datum::Int(n) => CellVal::Int(n),
            Datum::UInt(n) => CellVal::UInt(n),
            Datum::Float(n) => CellVal::Float(n),
            Datum::Str(s) => match <[u8; 3]>::try_from(s.as_bytes()) {
                Ok(hash) => CellVal::Hash(hash),
                Err(_) => CellVal::Str(s.to_string()),
            },
            Datum::Json(v) => CellVal::Json(v.clone()),
        }
    }
}
