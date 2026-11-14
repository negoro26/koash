use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Sql;

static SIGNATURE: Signature = Signature {
    name: "@sql",
    aliases: &[],
    description: "Run SQLite sub-kernel query",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Sql {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        crate::subkernel::run_sql_inprocess(session, &args.join(" "))
    }
}
