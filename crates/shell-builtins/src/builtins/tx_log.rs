use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct TxLog;

static SIGNATURE: Signature = Signature {
    name: "tx-log",
    aliases: &["tx_log"],
    description: "List recorded transactional micro-commits",
    flags: &[],
    columns: crate::builtin::columns!["tx_id", "command", "files"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
};

impl Builtin for TxLog {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        _args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
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
                    tx.opaque_after
                        .clone()
                        .map_or(JsonVal::Null, JsonVal::String),
                );
                rec
            })
            .collect();
        Value::from_records(records)
    }
}
