use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use shell_syntax::{Span, SyntaxError};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;

pub type StructuredRecord = BTreeMap<String, serde_json::Value>;

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
            CellVal::Hash(h) => {
                serde_json::Value::String(String::from_utf8_lossy(h).into_owned())
            }
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
        let ncols = self.columns.len();
        if ncols == 0 {
            0
        } else {
            self.cells.len() / ncols
        }
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
    RawStream(Pin<Box<dyn AsyncRead + Send>>),
    RecordStream(mpsc::Receiver<StructuredRecord>),
    Error(StructuredDiagnostic),
}

impl Value {
    pub fn from_batch(batch: RecordBatch) -> Self {
        Value::Batch(batch)
    }

    pub fn from_records(records: Vec<StructuredRecord>) -> Self {
        Value::Batch(RecordBatch::from_maps(records))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Value::Bytes(bytes)
    }

    pub fn into_batch_sync(self) -> Result<RecordBatch, StructuredDiagnostic> {
        match self {
            Value::Empty => Ok(RecordBatch::empty()),
            Value::Error(diag) => Err(diag),
            Value::Batch(batch) => Ok(batch),
            Value::Bytes(bytes) => Ok(batch_from_raw_bytes(&bytes)),
            Value::RawStream(_) | Value::RecordStream(_) => Err(StructuredDiagnostic::new(
                "E_ASYNC_STREAM",
                "async stream requires async consumer",
                None,
                None,
            )),
        }
    }

    pub async fn into_batch(self) -> Result<RecordBatch, StructuredDiagnostic> {
        match self {
            Value::Empty => Ok(RecordBatch::empty()),
            Value::Error(diag) => Err(diag),
            Value::Batch(batch) => Ok(batch),
            Value::Bytes(bytes) => Ok(batch_from_raw_bytes(&bytes)),
            Value::RawStream(mut reader) => {
                let mut bytes = Vec::new();
                reader
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|e| StructuredDiagnostic::from_io("E_STREAM_READ", &e, None))?;
                Ok(batch_from_raw_bytes(&bytes))
            }
            Value::RecordStream(mut rx) => {
                let mut out = Vec::new();
                while let Some(rec) = rx.recv().await {
                    out.push(rec);
                }
                Ok(RecordBatch::from_maps(out))
            }
        }
    }

    pub async fn into_records(self) -> Result<Vec<StructuredRecord>, StructuredDiagnostic> {
        Ok(self.into_batch().await?.to_maps())
    }

    pub fn into_raw_bytes_sync(self) -> Result<Vec<u8>, StructuredDiagnostic> {
        match self {
            Value::Empty => Ok(Vec::new()),
            Value::Error(diag) => Err(diag),
            Value::Bytes(bytes) => Ok(bytes),
            Value::Batch(batch) => Ok(batch_to_ndjson_bytes(&batch)),
            Value::RawStream(_) | Value::RecordStream(_) => Err(StructuredDiagnostic::new(
                "E_ASYNC_STREAM",
                "async stream requires async consumer",
                None,
                None,
            )),
        }
    }

    pub async fn into_raw_bytes(self) -> Result<Vec<u8>, StructuredDiagnostic> {
        match self {
            Value::Empty => Ok(Vec::new()),
            Value::Error(diag) => Err(diag),
            Value::Bytes(bytes) => Ok(bytes),
            Value::Batch(batch) => Ok(batch_to_ndjson_bytes(&batch)),
            Value::RawStream(mut reader) => {
                let mut buf = Vec::new();
                reader
                    .read_to_end(&mut buf)
                    .await
                    .map_err(|e| StructuredDiagnostic::from_io("E_STREAM_READ", &e, None))?;
                Ok(buf)
            }
            Value::RecordStream(mut rx) => {
                let mut out = Vec::new();
                while let Some(rec) = rx.recv().await {
                    if let Ok(json) = serde_json::to_string(&rec) {
                        out.extend_from_slice(json.as_bytes());
                        out.push(b'\n');
                    }
                }
                Ok(out)
            }
        }
    }

    pub fn render_tty_sync(self, writer: &mut dyn Write, color: bool) -> Result<(), StructuredDiagnostic> {
        match self {
            Value::Empty => Ok(()),
            Value::Error(diag) => Err(diag),
            Value::Bytes(bytes) => {
                let _ = writer.write_all(&bytes);
                let _ = writer.flush();
                Ok(())
            }
            Value::Batch(batch) => {
                render_batch_table(&batch, writer, color);
                Ok(())
            }
            Value::RawStream(_) | Value::RecordStream(_) => Err(StructuredDiagnostic::new(
                "E_ASYNC_STREAM",
                "async stream requires async renderer",
                None,
                None,
            )),
        }
    }

    pub async fn render_tty(self, writer: &mut dyn Write, color: bool) -> Result<(), StructuredDiagnostic> {
        match self {
            Value::Empty | Value::Bytes(_) | Value::Batch(_) | Value::Error(_) => {
                self.render_tty_sync(writer, color)
            }
            Value::RawStream(mut reader) => {
                let mut buf = [0u8; 8192];
                loop {
                    let n = reader
                        .read(&mut buf)
                        .await
                        .map_err(|e| StructuredDiagnostic::from_io("E_STREAM_READ", &e, None))?;
                    if n == 0 {
                        break;
                    }
                    let _ = writer.write_all(&buf[..n]);
                }
                let _ = writer.flush();
                Ok(())
            }
            Value::RecordStream(mut rx) => {
                let mut records = Vec::new();
                while let Some(rec) = rx.recv().await {
                    records.push(rec);
                }
                render_records_table(&records, writer, color);
                Ok(())
            }
        }
    }
}

fn batch_from_raw_bytes(bytes: &[u8]) -> RecordBatch {
    let text = String::from_utf8_lossy(bytes);
    let mut records = Vec::new();
    for line in text.lines() {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(line) {
            let rec: StructuredRecord = map.into_iter().collect();
            records.push(rec);
        } else {
            let mut rec = StructuredRecord::new();
            rec.insert("line".to_string(), serde_json::Value::String(line.to_string()));
            records.push(rec);
        }
    }
    RecordBatch::from_maps(records)
}

fn batch_to_ndjson_bytes(batch: &RecordBatch) -> Vec<u8> {
    let maps = batch.to_maps();
    let mut out = Vec::new();
    for rec in &maps {
        if let Ok(json) = serde_json::to_string(rec) {
            out.extend_from_slice(json.as_bytes());
            out.push(b'\n');
        }
    }
    out
}

pub fn render_records_table(records: &[StructuredRecord], writer: &mut dyn Write, color: bool) {
    if records.is_empty() {
        return;
    }
    let batch = RecordBatch::from_maps(records.to_vec());
    render_batch_table(&batch, writer, color);
}

pub fn render_batch_table(batch: &RecordBatch, writer: &mut dyn Write, color: bool) {
    let ncols = batch.ncols();
    let nrows = batch.nrows();
    if ncols == 0 || nrows == 0 {
        return;
    }

    let mut widths: Vec<usize> = batch.columns.iter().map(|c| c.len()).collect();
    let mut scratch = String::new();
    for r in 0..nrows {
        let row = batch.row(r);
        for c in 0..ncols {
            let len = row[c].display_len(&mut scratch);
            if len > widths[c] {
                widths[c] = len;
            }
        }
    }

    let mut bw = BufWriter::with_capacity(8192, writer);
    for (i, col) in batch.columns.iter().enumerate() {
        if i > 0 {
            let _ = bw.write_all(b"  ");
        }
        if color {
            let _ = bw.write_all(b"\x1b[1;36m");
            let _ = bw.write_all(col.as_bytes());
            let _ = bw.write_all(b"\x1b[0m");
        } else {
            let _ = bw.write_all(col.as_bytes());
        }
        let pad = widths[i].saturating_sub(col.len());
        write_spaces(&mut bw, pad);
    }
    let _ = bw.write_all(b"\n");

    for (i, &w) in widths.iter().enumerate() {
        if i > 0 {
            let _ = bw.write_all(b"  ");
        }
        write_dashes(&mut bw, w);
    }
    let _ = bw.write_all(b"\n");

    for r in 0..nrows {
        let row = batch.row(r);
        for (c, cell) in row.iter().enumerate() {
            if c > 0 {
                let _ = bw.write_all(b"  ");
            }
            let written = cell.write_to(&mut bw, &mut scratch);
            let pad = widths[c].saturating_sub(written);
            write_spaces(&mut bw, pad);
        }
        let _ = bw.write_all(b"\n");
    }
    let _ = bw.flush();
}

fn write_spaces<W: Write>(w: &mut W, mut count: usize) {
    const SPACES: &[u8; 64] = b"                                                                ";
    while count > 0 {
        let n = count.min(SPACES.len());
        let _ = w.write_all(&SPACES[..n]);
        count -= n;
    }
}

fn write_dashes<W: Write>(w: &mut W, mut count: usize) {
    const DASHES: &[u8; 64] = b"----------------------------------------------------------------";
    while count > 0 {
        let n = count.min(DASHES.len());
        let _ = w.write_all(&DASHES[..n]);
        count -= n;
    }
}

pub fn format_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
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

pub fn batch_to_compact_sigil(batch: &RecordBatch) -> String {
    if batch.is_empty() {
        return String::new();
    }
    let mut out = String::from("@schema:");
    for (i, col) in batch.columns.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        out.push_str(col.as_ref());
    }
    out.push('\n');
    for r in 0..batch.nrows() {
        let row = batch.row(r);
        for (c, cell) in row.iter().enumerate() {
            if c > 0 {
                out.push('¦');
            }
            if !matches!(cell, CellVal::Null) {
                let s = cell.format_to_string().replace('\n', "\\n");
                out.push_str(&s);
            }
        }
        out.push('\n');
    }
    out
}

pub fn records_to_compact_sigil(records: &[StructuredRecord]) -> String {
    if records.is_empty() {
        return String::new();
    }
    let batch = RecordBatch::from_maps(records.to_vec());
    batch_to_compact_sigil(&batch)
}
