use nix::sys::signal::{Signal, kill as nix_kill};
use nix::unistd::Pid;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use super::common::usage_error;
use super::fs_ops::fail;
use crate::args::Args;
use crate::builtin::{Builtin, Flag, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Kill;

static SIGNATURE: Signature = Signature {
    name: "kill",
    aliases: &[],
    description: "Send signal to process",
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
        let pids = args
            .operands
            .iter()
            .map(|operand| {
                operand.parse::<i32>().map(Pid::from_raw).map_err(|_| {
                    usage_error(
                        format!("kill: invalid process id '{operand}'"),
                        "kill [-s SIGNAL | -N] PID...",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if pids.is_empty() {
            return Err(usage_error(
                "kill requires at least one PID",
                "kill [-s SIGNAL | -N] PID...",
            ));
        }

        for pid in pids {
            if let Err(e) = nix_kill(pid, sig) {
                fail(session, "E_KILL", format!("kill: ({pid}): {}", e.desc()));
            }
        }
        Ok(Value::Empty)
    }
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
