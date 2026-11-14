use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value, batch_to_compact_sigil};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct ToFormat;

static SIGNATURE: Signature = Signature {
    name: "to",
    aliases: &[],
    description: "Serialize structured records (e.g. to json, to compact)",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for ToFormat {
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
        let batch = input.into_batch()?;
        match fmt {
            "json" => {
                let mut json = serde_json::to_string_pretty(&batch.records())
                    .map_err(|e| StructuredDiagnostic::new("E_JSON", e.to_string(), None, None))?;
                json.push('\n');
                Ok(Value::from_bytes(json.into_bytes()))
            }
            "compact" => {
                let sigil = batch_to_compact_sigil(&batch);
                Ok(Value::from_bytes(sigil.into_bytes()))
            }
            other => Err(StructuredDiagnostic::new(
                "E_ARGS",
                format!("unsupported `to` format `{other}`"),
                None,
                None,
            )),
        }
    }
}
