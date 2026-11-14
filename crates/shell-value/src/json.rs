use std::collections::BTreeMap;

use crate::RecordBatch;

pub type StructuredRecord = BTreeMap<String, serde_json::Value>;

pub(crate) fn batch_from_raw_bytes(bytes: &[u8]) -> RecordBatch {
    let text = String::from_utf8_lossy(bytes);
    let mut records = Vec::new();
    for line in text.lines() {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(line)
        {
            let rec: StructuredRecord = map.into_iter().collect();
            records.push(rec);
        } else {
            let mut rec = StructuredRecord::new();
            rec.insert(
                "line".to_string(),
                serde_json::Value::String(line.to_string()),
            );
            records.push(rec);
        }
    }
    RecordBatch::from_maps(records)
}

pub(crate) fn batch_to_ndjson_bytes(batch: &RecordBatch) -> Vec<u8> {
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

pub fn format_cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
