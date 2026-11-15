use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::sole_stage_only;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};
use crate::sink::Sink;

pub(super) struct Exit;

static SIGNATURE: Signature = Signature {
    name: "exit",
    aliases: &[],
    description: "Exit the shell with a status",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::SoleStage,
    tx: TxPolicy::Track,
};

impl Builtin for Exit {
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
        let code = args
            .first()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(session.last_status);
        session.should_exit = Some(code);
        code
    }
}
