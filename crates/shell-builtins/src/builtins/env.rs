use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Env;

static SIGNATURE: Signature = Signature {
    name: "env",
    aliases: &[],
    description: "List environment variables",
    flags: &[],
    columns: crate::builtin::columns!["key", "value"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Env {
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
            .env
            .materialize()
            .into_iter()
            .map(|(k, v)| {
                let mut rec = StructuredRecord::new();
                rec.insert("key".to_string(), JsonVal::String(k));
                rec.insert("value".to_string(), JsonVal::String(v));
                rec
            })
            .collect();
        Value::from_records(records)
    }
}
