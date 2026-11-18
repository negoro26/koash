use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use shell_kernel::{JobState, KillSwitch, Session};
use shell_value::{StructuredDiagnostic, Value};

use super::common::sole_stage_only;
use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};
use crate::pipeline::KILLED_STATUS;
use crate::sink::Sink;

const INTERRUPTED_STATUS: i32 = 130;
const POLL_MIN: Duration = Duration::from_millis(1);
const POLL_MAX: Duration = Duration::from_millis(20);

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn note_interrupt(_: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

struct InterruptGuard(Option<SigAction>);

impl InterruptGuard {
    fn install(enabled: bool) -> Self {
        INTERRUPTED.store(false, Ordering::SeqCst);
        let action = SigAction::new(
            SigHandler::Handler(note_interrupt),
            SaFlags::empty(),
            SigSet::empty(),
        );
        let previous = if enabled {
            unsafe { sigaction(Signal::SIGINT, &action) }.ok()
        } else {
            None
        };
        Self(previous)
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        if let Some(previous) = &self.0 {
            unsafe {
                let _ = sigaction(Signal::SIGINT, previous);
            }
        }
    }
}

enum Outcome {
    Finished(i32),
    Stopped(i32),
    Interrupted,
    Killed,
}

pub(super) struct Wait;

static SIGNATURE: Signature = Signature {
    name: "wait",
    aliases: &[],
    description: "Wait for background jobs to finish",
    flags: &[],
    columns: &[],
    input: InputMode::None,
    placement: Placement::SoleStage,
    tx: TxPolicy::Track,
    strict_options: false,
};

impl Builtin for Wait {
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
        let _guard = InterruptGuard::install(session.interactive);
        if args.is_empty() {
            let running: Vec<usize> = session
                .jobs
                .iter()
                .filter(|j| j.state == JobState::Running)
                .map(|j| j.id)
                .collect();
            for id in running {
                match wait_for(session, id) {
                    Outcome::Finished(_) => {}
                    Outcome::Stopped(code) => return report_stopped(session, sink, id, code),
                    Outcome::Interrupted => return INTERRUPTED_STATUS,
                    Outcome::Killed => return KILLED_STATUS,
                }
            }
            return 0;
        }
        let mut status = 0;
        for arg in args {
            let id = match resolve(session, arg) {
                Target::Job(id) => id,
                Target::Finished(code) => {
                    status = code;
                    continue;
                }
                Target::Unknown => {
                    sink.stderr_line(&format!("wait: {arg}: not a job or child of this shell"));
                    status = 127;
                    continue;
                }
            };
            match wait_for(session, id) {
                Outcome::Finished(code) => status = code,
                Outcome::Stopped(code) => return report_stopped(session, sink, id, code),
                Outcome::Interrupted => return INTERRUPTED_STATUS,
                Outcome::Killed => return KILLED_STATUS,
            }
        }
        status
    }
}

enum Target {
    Job(usize),
    Finished(i32),
    Unknown,
}

fn resolve(session: &Session, operand: &str) -> Target {
    let finished = |matches: &dyn Fn(&(usize, Pid, i32)) -> bool| {
        session
            .finished_jobs
            .iter()
            .rev()
            .find(|entry| matches(entry))
            .map_or(Target::Unknown, |&(_, _, code)| Target::Finished(code))
    };
    if let Some(id) = operand.strip_prefix('%') {
        return match id.parse() {
            Ok(id) if session.jobs.iter().any(|j| j.id == id) => Target::Job(id),
            Ok(id) => finished(&|&(job, _, _)| job == id),
            Err(_) => Target::Unknown,
        };
    }
    let Ok(pid) = operand.parse().map(Pid::from_raw) else {
        return Target::Unknown;
    };
    match session.jobs.iter().find(|j| j.pids.contains(&pid)) {
        Some(job) => Target::Job(job.id),
        None => finished(&|&(_, finished, _)| finished == pid),
    }
}

fn report_stopped(session: &Session, sink: &Sink, id: usize, code: i32) -> i32 {
    let command = session
        .jobs
        .iter()
        .find(|j| j.id == id)
        .map(|j| j.command.as_str())
        .unwrap_or_default();
    sink.stderr_line(&format!("\n[{id}]+ Stopped                 {command}"));
    code
}

fn wait_for(session: &mut Session, id: usize) -> Outcome {
    let Some(job) = session.jobs.iter().find(|j| j.id == id) else {
        return Outcome::Finished(127);
    };
    let (pids, last_pid) = (job.pids.clone(), job.last_pid);
    let reaped = last_pid
        .filter(|last| !pids.contains(last))
        .and(job.last_status);
    let outcome = match wait_pids(&pids, last_pid, &session.kill) {
        Outcome::Finished(code) => Outcome::Finished(reaped.unwrap_or(code)),
        other => other,
    };
    match outcome {
        Outcome::Finished(code) => {
            session.jobs.retain(|j| j.id != id);
            if let Some(last) = last_pid {
                session.finished_jobs.push((id, last, code));
            }
        }
        Outcome::Stopped(_) => {
            if let Some(job) = session.jobs.iter_mut().find(|j| j.id == id) {
                job.state = JobState::Stopped;
            }
        }
        Outcome::Interrupted | Outcome::Killed => {}
    }
    outcome
}

fn wait_pids(pids: &[Pid], last_pid: Option<Pid>, kill: &KillSwitch) -> Outcome {
    let mut status = 127;
    for &pid in pids {
        let mut delay = POLL_MIN;
        loop {
            if INTERRUPTED.swap(false, Ordering::SeqCst) {
                return Outcome::Interrupted;
            }
            if kill.is_killed() {
                return Outcome::Killed;
            }
            let code = match waitpid(pid, Some(WaitPidFlag::WUNTRACED | WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::StillAlive) | Err(Errno::EINTR) => {
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(POLL_MAX);
                    continue;
                }
                Ok(WaitStatus::Exited(_, code)) => code,
                Ok(WaitStatus::Signaled(_, sig, _)) => 128 + sig as i32,
                Ok(WaitStatus::Stopped(_, sig)) => return Outcome::Stopped(128 + sig as i32),
                Ok(_) => continue,
                Err(_) => break,
            };
            if Some(pid) == last_pid {
                status = code;
            }
            break;
        }
    }
    Outcome::Finished(status)
}
