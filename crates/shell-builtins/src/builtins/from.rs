use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{
    Column, Datum, DynRows, RecordBatch, Schema, StructuredDiagnostic, StructuredRecord, Value,
};

use super::common::text_lines;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct FromFormat;

static SIGNATURE: Signature = Signature {
    name: "from",
    aliases: &[],
    description: "Parse structured input (from json, csv, tsv, lines)",
    flags: &[],
    columns: &[],
    input: InputMode::Bytes,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for FromFormat {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let fmt = args.first().map(|s| s.as_str()).unwrap_or("json");
        let bytes = input.into_raw_bytes();
        match fmt {
            "json" => parse_json(&bytes),
            "csv" => parse_delimited(&bytes, ','),
            "tsv" => parse_delimited(&bytes, '\t'),
            "lines" => parse_lines(&bytes),
            other => Err(StructuredDiagnostic::new(
                "E_ARGS",
                format!("unsupported `from` format `{other}`"),
                None,
                Some("formats: json, csv, tsv, lines".to_string()),
            )),
        }
    }
}

fn parse_json(bytes: &[u8]) -> Result<Value, StructuredDiagnostic> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::from_batch(RecordBatch::empty()));
    }
    if let Ok(val) = serde_json::from_str::<JsonVal>(trimmed) {
        match val {
            JsonVal::Array(arr) => {
                let mut objs = Vec::with_capacity(arr.len());
                for v in arr {
                    if let JsonVal::Object(m) = v {
                        objs.push(m);
                    }
                }
                return Ok(Value::from_batch(batch_from_json_objects(objs)?));
            }
            JsonVal::Object(map) => {
                return Ok(Value::from_batch(batch_from_json_objects(vec![map])?));
            }
            _ => {}
        }
    }
    let mut objs = Vec::new();
    for line in trimmed.lines() {
        if let Ok(JsonVal::Object(map)) = serde_json::from_str::<JsonVal>(line) {
            objs.push(map);
        }
    }
    Ok(Value::from_batch(batch_from_json_objects(objs)?))
}

fn parse_lines(bytes: &[u8]) -> Result<Value, StructuredDiagnostic> {
    static COLS: &[Column] = &[Column::new("line").text()];
    let mut rows = DynRows::new(Schema::of(COLS));
    let mut any = false;
    for line in text_lines(bytes) {
        any = true;
        let text = String::from_utf8_lossy(line);
        rows.push_with(|sink| sink.put_str(&text));
    }
    if !any {
        return Ok(Value::from_batch(RecordBatch::empty()));
    }
    Ok(Value::from_batch(rows.finish()?))
}

fn parse_delimited(bytes: &[u8], delim: char) -> Result<Value, StructuredDiagnostic> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let records = split_delimited_records(&text, delim);
    let Some((header, data)) = records.split_first() else {
        return Ok(Value::from_batch(RecordBatch::empty()));
    };
    if header.is_empty() {
        return Ok(Value::from_batch(RecordBatch::empty()));
    }
    let schema = Schema::named(header.iter().map(String::as_str));
    let mut rows = DynRows::new(schema);
    let ncols = header.len();
    for rec in data {
        rows.push_with(|sink| {
            for col in 0..ncols {
                match rec.get(col) {
                    Some(val) => put_inferred(sink, val),
                    None => sink.put(Datum::Null),
                }
            }
        });
    }
    Ok(Value::from_batch(rows.finish()?))
}

fn put_inferred(sink: &mut shell_value::RowSink<'_>, raw: &str) {
    if raw.is_empty() {
        sink.put_str("");
    } else if raw == "true" {
        sink.put(Datum::Bool(true));
    } else if raw == "false" {
        sink.put(Datum::Bool(false));
    } else if let Ok(u) = raw.parse::<u64>() {
        sink.put(Datum::UInt(u));
    } else if let Ok(i) = raw.parse::<i64>() {
        sink.put(Datum::Int(i));
    } else if raw.contains(['.', 'e', 'E'])
        && let Ok(f) = raw.parse::<f64>()
        && f.is_finite()
    {
        sink.put(Datum::Float(f));
    } else {
        sink.put_str(raw);
    }
}

fn split_delimited_records(input: &str, delim: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut quoted = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                in_quotes = true;
                quoted = true;
            }
            ch if ch == delim => {
                row.push(std::mem::take(&mut field));
            }
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                if row.len() > 1 || quoted || !row[0].is_empty() {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
                quoted = false;
            }
            ch => field.push(ch),
        }
    }
    if !field.is_empty() || !row.is_empty() || quoted {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn batch_from_json_objects(
    objs: Vec<serde_json::Map<String, JsonVal>>,
) -> Result<RecordBatch, StructuredDiagnostic> {
    let records = objs
        .into_iter()
        .map(|map| map.into_iter().collect::<StructuredRecord>())
        .collect();
    RecordBatch::from_maps(records)
}
