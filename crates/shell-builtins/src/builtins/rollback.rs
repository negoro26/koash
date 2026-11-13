use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Rollback;

static SIGNATURE: Signature = Signature {
    name: "rollback",
    aliases: &[],
    description: "Roll back filesystem state to a specific transaction ID",
    flags: &[],
    columns: &["tx_id", "restored_files"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Skip,
};

impl Builtin for Rollback {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
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
}
