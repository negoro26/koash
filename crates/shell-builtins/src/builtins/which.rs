use serde_json::Value as JsonVal;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Which;

static SIGNATURE: Signature = Signature {
    name: "which",
    aliases: &[],
    description: "Locate executable in PATH",
    flags: &[],
    columns: &["name", "path"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Which {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let path_var = session.env.get("PATH").unwrap_or_default();

        let mut records = Vec::new();
        for cmd in args {
            let mut found = None;
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(cmd);
                if candidate.is_file() {
                    found = Some(candidate.to_string_lossy().to_string());
                    break;
                }
            }
            if let Some(p) = found {
                let mut rec = StructuredRecord::new();
                rec.insert("name".to_string(), JsonVal::String(cmd.clone()));
                rec.insert("path".to_string(), JsonVal::String(p));
                records.push(rec);
            }
        }
        Ok(Value::from_records(records))
    }
}
