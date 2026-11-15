use std::borrow::Cow;
use std::sync::Arc;

use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{CellVal, RecordBatch, StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct FromFormat;

static SIGNATURE: Signature = Signature {
    name: "from",
    aliases: &[],
    description: "Parse structured input (e.g. from json)",
    flags: &[],
    columns: &[],
    input: InputMode::Bytes,
    placement: Placement::Any,
    tx: TxPolicy::Track,
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
        if fmt != "json" {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                format!("unsupported `from` format `{fmt}`"),
                None,
                None,
            ));
        }
        let bytes = input.into_raw_bytes();
        let text = String::from_utf8_lossy(&bytes);
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
                    return Ok(Value::from_batch(batch_from_json_objects(objs)));
                }
                JsonVal::Object(map) => {
                    return Ok(Value::from_batch(batch_from_json_objects(vec![map])));
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
        Ok(Value::from_batch(batch_from_json_objects(objs)))
    }
}

fn batch_from_json_objects(objs: Vec<serde_json::Map<String, JsonVal>>) -> RecordBatch {
    if objs.is_empty() {
        return RecordBatch::empty();
    }
    let mut columns: Vec<Cow<'static, str>> = Vec::new();
    for map in &objs {
        for k in map.keys() {
            if !columns.iter().any(|c| c.as_ref() == k.as_str()) {
                columns.push(Cow::Owned(k.clone()));
            }
        }
    }
    let ncols = columns.len();
    let mut cells = Vec::with_capacity(objs.len() * ncols);
    for mut map in objs {
        for col in &columns {
            let cell = match map.remove(col.as_ref()) {
                Some(v) => CellVal::from_json(v),
                None => CellVal::Null,
            };
            cells.push(cell);
        }
    }
    RecordBatch {
        columns: Arc::from(columns),
        cells,
    }
}
