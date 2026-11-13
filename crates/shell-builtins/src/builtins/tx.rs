use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

pub(crate) fn undo(session: &mut Session) -> Result<Value, StructuredDiagnostic> {
    match session.undo_last()? {
        Some((tx_id, count)) => {
            let mut rec = StructuredRecord::new();
            rec.insert("tx_id".to_string(), JsonVal::String(tx_id));
            rec.insert("restored_files".to_string(), json!(count));
            Ok(Value::from_records(vec![rec]))
        }
        None => Ok(Value::from_records(Vec::new())),
    }
}

pub(crate) fn rollback(session: &mut Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let Some(tx_id) = args.first() else {
        return Err(StructuredDiagnostic::new(
            "E_ARGS",
            "rollback requires a transaction ID (e.g. tx_001)",
            None,
            None,
        ));
    };
    let count = session.rollback_tx(tx_id)?;
    let mut rec = StructuredRecord::new();
    rec.insert("tx_id".to_string(), JsonVal::String(tx_id.clone()));
    rec.insert("restored_files".to_string(), json!(count));
    Ok(Value::from_records(vec![rec]))
}

pub(crate) fn tx_log(session: &Session) -> Result<Value, StructuredDiagnostic> {
    let records = session
        .tx_log
        .iter()
        .map(|tx| {
            let mut rec = StructuredRecord::new();
            rec.insert("tx_id".to_string(), JsonVal::String(tx.id.clone()));
            rec.insert("command".to_string(), JsonVal::String(tx.command.clone()));
            rec.insert("files".to_string(), json!(tx.preimages.len()));
            rec.insert(
                "opaque_after".to_string(),
                tx.opaque_after.clone().map_or(JsonVal::Null, JsonVal::String),
            );
            rec
        })
        .collect();
    Ok(Value::from_records(records))
}
