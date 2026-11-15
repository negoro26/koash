use nix::sys::signal::{Signal, killpg};
use shell_kernel::JobState;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::job_index;
use super::common::sole_stage_only;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};
use crate::sink::Sink;

pub(super) struct Bg;

static SIGNATURE: Signature = Signature {
    name: "bg",
    aliases: &[],
    description: "Resume a stopped job in the background",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::SoleStage,
    tx: TxPolicy::Track,
};

impl Builtin for Bg {
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

    fn run_alone(&self, session: &mut Session, args: &[String], sink: &Sink) -> i32 {
        let default = session
            .jobs
            .iter()
            .rposition(|j| j.state == JobState::Stopped);
        let Some(idx) = job_index(session, args.first(), default) else {
            sink.stderr_line("bg: no stopped job");
            return 1;
        };
        let job = &mut session.jobs[idx];
        job.state = JobState::Running;
        let _ = killpg(job.pgid, Some(Signal::SIGCONT));
        sink.stderr_line(&format!("[{}]+ {} &", job.id, job.command));
        0
    }
}
