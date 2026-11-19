use nix::sys::signal::{Signal, kill as nix_kill, killpg};
use nix::unistd::Pid;
use shell_kernel::{JobState, Session};
use shell_value::{StructuredDiagnostic, Value};

use super::common::usage_error;
use super::fs_ops::fail;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Kill;

static SIGNATURE: Signature = Signature {
    name: "kill",
    aliases: &[],
    description: "Send a signal to a process or a job (%N)",
    flags: &[Flag::valued(
        's',
        "signal",
        "signal name or number; -9 and -15 are shorthand",
    )
    .numeric()],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
    strict_options: true,
};

impl Builtin for Kill {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let args = Args::parse(&SIGNATURE, args)?;
        let sig = match args.value("s") {
            Some(name) => parse_signal(name)?,
            None => Signal::SIGTERM,
        };
        let targets = args
            .operands
            .iter()
            .map(|operand| parse_target(operand))
            .collect::<Result<Vec<_>, _>>()?;
        if targets.is_empty() {
            return Err(usage_error("kill requires at least one PID or %JOB", USAGE));
        }

        for (operand, target) in args.operands.iter().zip(targets) {
            if let Err(what) = deliver(session, target, sig) {
                fail(session, "E_KILL", format!("kill: ({operand}): {what}"));
            }
        }
        Ok(Value::Empty)
    }
}

const USAGE: &str = "kill [-s SIGNAL | -N] PID|%JOB...";

enum Target {
    Process(Pid),
    Job(usize),
}

fn parse_target(operand: &str) -> Result<Target, StructuredDiagnostic> {
    let invalid = || {
        usage_error(
            format!("kill: invalid process id or job '{operand}'"),
            USAGE,
        )
    };
    match operand.strip_prefix('%') {
        Some(id) => id.parse().map(Target::Job).map_err(|_| invalid()),
        None => operand
            .parse()
            .map(|pid| Target::Process(Pid::from_raw(pid)))
            .map_err(|_| invalid()),
    }
}

fn deliver(session: &Session, target: Target, sig: Signal) -> Result<(), &'static str> {
    let id = match target {
        Target::Process(pid) => return nix_kill(pid, sig).map_err(|e| e.desc()),
        Target::Job(id) => id,
    };
    let job = session
        .jobs
        .iter()
        .find(|j| j.id == id)
        .ok_or("no such job")?;
    let send = |sig: Signal| {
        if job.pgid != session.shell_pgid {
            return killpg(job.pgid, sig);
        }
        job.pids.iter().try_for_each(|&pid| nix_kill(pid, sig))
    };
    send(sig).map_err(|e| e.desc())?;
    if job.state == JobState::Stopped && !matches!(sig, Signal::SIGCONT | Signal::SIGKILL) {
        let _ = send(Signal::SIGCONT);
    }
    Ok(())
}

fn parse_signal(name: &str) -> Result<Signal, StructuredDiagnostic> {
    let unknown =
        || StructuredDiagnostic::new("E_SIGNAL", format!("unknown signal `{name}`"), None, None);
    if let Ok(number) = name.parse::<i32>() {
        return Signal::try_from(number).map_err(|_| unknown());
    }
    match name.to_uppercase().trim_start_matches("SIG") {
        "INT" => Ok(Signal::SIGINT),
        "TERM" => Ok(Signal::SIGTERM),
        "KILL" => Ok(Signal::SIGKILL),
        "HUP" => Ok(Signal::SIGHUP),
        "STOP" => Ok(Signal::SIGSTOP),
        "CONT" => Ok(Signal::SIGCONT),
        _ => Err(unknown()),
    }
}
