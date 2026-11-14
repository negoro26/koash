use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::sole_stage_only;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};
use crate::sink::Sink;

pub(super) struct Unset;

static SIGNATURE: Signature = Signature {
    name: "unset",
    aliases: &[],
    description: "Remove environment variables",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::SoleStage,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Unset {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        _args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        Err(sole_stage_only(&SIGNATURE))
    }

    fn run_alone(&self, session: &mut Session, args: &[String], _sink: &Sink) -> i32 {
        for arg in args {
            session.env.remove(arg);
        }
        0
    }
}
