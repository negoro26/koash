use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Prewalk;

static SIGNATURE: Signature = Signature {
    name: "prewalk",
    aliases: &[],
    description: "Emit token-budgeted structural repository skeleton",
    flags: &["--budget"],
    columns: &["file", "rank", "tokens", "skeleton"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Prewalk {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        crate::code_intel::run_prewalk(session, args)
    }
}
