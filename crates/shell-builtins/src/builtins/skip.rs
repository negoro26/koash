use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::usage_error;
use crate::args::Args;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Skip;

static SIGNATURE: Signature = Signature {
    name: "skip",
    aliases: &[],
    description: "Skip the first N records",
    flags: &[],
    columns: &[],
    input: InputMode::Table,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Skip {
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
        let n = match parsed.operands.as_slice() {
            [] => 1usize,
            [raw] => raw
                .parse::<usize>()
                .map_err(|_| usage_error(format!("skip: invalid count `{raw}`"), "skip [N]"))?,
            _ => return Err(usage_error("skip takes at most one count", "skip [N]")),
        };
        let mut batch = input.into_batch()?;
        batch.skip_rows(n);
        Ok(Value::from_batch(batch))
    }
}
