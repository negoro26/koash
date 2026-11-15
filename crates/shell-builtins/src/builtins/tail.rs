use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::operands_read_stdin;
use super::slice::{self, End};
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Tail;

static SIGNATURE: Signature = Signature {
    name: "tail",
    aliases: &[],
    description: "Take last N records, file lines, or raw lines",
    flags: &slice::FLAGS,
    columns: &["file", "line", "hash", "text"],
    input: InputMode::Lines,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Tail {
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
        slice::run(session, &SIGNATURE, End::Tail, args, input)
    }
}
