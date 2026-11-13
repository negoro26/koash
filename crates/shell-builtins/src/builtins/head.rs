use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::operands_read_stdin;
use super::slice::{self, End};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Head;

static SIGNATURE: Signature = Signature {
    name: "head",
    aliases: &[],
    description: "Take first N records, file lines, or raw lines",
    flags: &slice::FLAGS,
    columns: &["file", "line", "hash", "text"],
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Head {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn reads_input(&self, args: &[String]) -> bool {
        operands_read_stdin(&SIGNATURE, args)
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        slice::run(session, &SIGNATURE, End::Head, args, input)
    }
}
