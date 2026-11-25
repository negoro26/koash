use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Reverse;

static SIGNATURE: Signature = Signature {
    name: "reverse",
    aliases: &[],
    description: "Reverse the order of structured records",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Reverse {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let parsed = Args::parse(&SIGNATURE, args)?;
        if !parsed.operands.is_empty() {
            return Err(usage_error("reverse takes no arguments", "reverse"));
        }
        let mut batch = input.into_batch()?;
        batch.reverse_rows();
        Ok(Value::from_batch(batch))
    }
}
