use nix::sys::signal::{Signal, killpg};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::job_index;
use super::common::sole_stage_only;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};
use crate::sink::Sink;
use crate::wiring::finish_foreground;

pub(super) struct Fg;

static SIGNATURE: Signature = Signature {
    name: "fg",
    aliases: &[],
    description: "Resume a job in the foreground",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::SoleStage,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Fg {
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
        let default = session.jobs.len().checked_sub(1);
        let Some(idx) = job_index(session, args.first(), default) else {
            sink.stderr_line("fg: no current job");
            return 1;
        };
        let job = session.jobs[idx].clone();
        sink.stderr_line(&job.command);
        session.give_terminal(job.pgid);
        session.kill.track(job.pgid);
        let _ = killpg(job.pgid, Some(Signal::SIGCONT));
        let status = finish_foreground(
            session,
            sink,
            job.pgid,
            &job.pids,
            job.pids.last().copied(),
            &job.command,
            Some(job.id),
        );
        session.kill.untrack(job.pgid);
        status
    }
}
