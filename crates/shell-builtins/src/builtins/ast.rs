use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Ast;

static SIGNATURE: Signature = Signature {
    name: "ast",
    aliases: &[],
    description: "Structural symbol and definition search with Hashline anchors",
    flags: &[],
    columns: crate::builtin::columns!["file", "line", "hash", "kind", "text"],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Ast {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        crate::code_intel::run_ast(session, &SIGNATURE, args)
    }
}
