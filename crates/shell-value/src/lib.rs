mod batch;
mod cell;
mod datum;
mod json;
mod rows;
mod schema;
mod text;
mod typed;

use std::collections::BTreeMap;
use std::io::Write;

use serde::{Deserialize, Serialize};
use shell_syntax::{Span, SyntaxError};
use thiserror::Error;

pub use batch::{RecordBatch, Row, RowOrder};
pub use datum::Datum;
pub use json::{Records, StructuredRecord};
pub use rows::{CellValue, DynRows, Record, RowSink, Rows};
pub use schema::{ColType, Column, Schema, column_index};
pub use text::{batch_to_compact_sigil, render_batch_table};
pub use typed::{FileMode, OrBlank, ShortText, UnixTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaintLevel {
    #[default]
    TrustedLocal,
    UntrustedExternal,
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{code}: {message}")]
pub struct StructuredDiagnostic {
    pub code: String,
    pub message: String,
    pub span: Option<Span>,
    pub hint: Option<String>,
}

impl StructuredDiagnostic {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        span: Option<Span>,
        hint: Option<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span,
            hint,
        }
    }

    pub fn from_io(code: impl Into<String>, err: &std::io::Error, span: Option<Span>) -> Self {
        Self {
            code: code.into(),
            message: err.to_string(),
            span,
            hint: None,
        }
    }
}

impl From<SyntaxError> for StructuredDiagnostic {
    fn from(err: SyntaxError) -> Self {
        Self {
            code: err.code,
            message: err.message,
            span: Some(err.span),
            hint: err.hint,
        }
    }
}

pub enum Value {
    Empty,
    Bytes(Vec<u8>),
    Batch(RecordBatch),
}

impl Value {
    pub fn from_batch(batch: RecordBatch) -> Self {
        Value::Batch(batch)
    }

    pub fn from_records(records: Vec<StructuredRecord>) -> Result<Self, StructuredDiagnostic> {
        Ok(Value::Batch(RecordBatch::from_maps(records)?))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Value::Bytes(bytes)
    }

    pub fn into_batch(self) -> Result<RecordBatch, StructuredDiagnostic> {
        match self {
            Value::Empty => Ok(RecordBatch::empty()),
            Value::Batch(batch) => Ok(batch),
            Value::Bytes(bytes) => json::batch_from_raw_bytes(&bytes),
        }
    }

    pub fn into_raw_bytes(self) -> Vec<u8> {
        match self {
            Value::Empty => Vec::new(),
            Value::Bytes(bytes) => bytes,
            Value::Batch(batch) => {
                let mut out = Vec::new();
                batch.write_ndjson(&mut out);
                out
            }
        }
    }

    pub fn render_tty(self, writer: &mut dyn Write, color: bool) {
        match self {
            Value::Empty => {}
            Value::Bytes(bytes) => {
                let _ = writer.write_all(&bytes);
                let _ = writer.flush();
            }
            Value::Batch(batch) => render_batch_table(&batch, writer, color),
        }
    }
}

pub fn hashline_bytes(line: &[u8]) -> [u8; 3] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
    let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
    let mut h: u32 = 0x811c_9dc5;
    for &b in trimmed {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    [
        HEX[((h >> 8) & 0xf) as usize],
        HEX[((h >> 4) & 0xf) as usize],
        HEX[(h & 0xf) as usize],
    ]
}

pub fn hashline_tag(line: &str) -> String {
    let bytes = hashline_bytes(line.as_bytes());
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn redact_secrets(text: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = text.to_string();
    for (k, v) in env {
        let upper = k.to_ascii_uppercase();
        if (upper.ends_with("_KEY")
            || upper.ends_with("_TOKEN")
            || upper.ends_with("_SECRET")
            || upper.ends_with("_PASSWORD"))
            && v.len() >= 8
            && out.contains(v.as_str())
        {
            out = out.replace(v.as_str(), "[REDACTED]");
        }
    }
    out
}
