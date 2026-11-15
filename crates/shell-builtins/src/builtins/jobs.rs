use shell_kernel::JobState;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::sole_stage_only;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};
use crate::sink::Sink;

pub(super) struct Jobs;

static SIGNATURE: Signature = Signature {
    name: "jobs",
    aliases: &[],
    description: "List background and stopped jobs",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::SoleStage,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Jobs {
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

    fn run_alone(&self, session: &mut Session, _args: &[String], sink: &Sink) -> i32 {
        let mut out = String::new();
        for job in &session.jobs {
            let state = match job.state {
                JobState::Running => "Running",
                JobState::Stopped => "Stopped",
                JobState::Done(_) => "Done",
            };
            out.push_str(&format!("[{}]+ {state:<23} {}\n", job.id, job.command));
        }
        if !out.is_empty() {
            sink.stdout(out.into_bytes());
        }
        0
    }
}
