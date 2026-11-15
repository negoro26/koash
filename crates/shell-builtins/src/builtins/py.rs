use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Py;

static SIGNATURE: Signature = Signature {
    name: "@py",
    aliases: &[],
    description: "Run Python sub-kernel snippet",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Py {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        session.mark_opaque("@py".to_string());
        crate::subkernel::run_worker(
            session,
            crate::subkernel::KernelKind::Python,
            &args.join(" "),
        )
    }
}
