use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Last;

static SIGNATURE: Signature = Signature {
    name: "last",
    aliases: &[],
    description: "Take last N records",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Last {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let n = args
            .first()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        let mut batch = input.into_batch()?;
        batch.tail_rows(n);
        Ok(Value::from_batch(batch))
    }
}
