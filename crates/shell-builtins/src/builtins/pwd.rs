use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Pwd;

static SIGNATURE: Signature = Signature {
    name: "pwd",
    aliases: &[],
    description: "Print current working directory",
    flags: &[],
    columns: &["path"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Pwd {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        _args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut rec = StructuredRecord::new();
        rec.insert(
            "path".to_string(),
            JsonVal::String(session.cwd.to_string_lossy().to_string()),
        );
        Ok(Value::from_records(vec![rec]))
    }
}
