use nix::sys::signal::{Signal, kill as nix_kill};
use nix::unistd::Pid;
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, Value};

use crate::builtin::{Builtin, InputMode, Placement, Signature, TxPolicy};

pub(super) struct Kill;

static SIGNATURE: Signature = Signature {
    name: "kill",
    aliases: &[],
    description: "Send signal to process",
    flags: &["-9", "-15", "-s"],
    columns: &[],
    input: InputMode::None,
    placement: Placement::Any,
    tx: TxPolicy::Track,
};

impl Builtin for Kill {
    fn signature(&self) -> &'static Signature {
        &SIGNATURE
    }

    fn run(
        &self,
        _session: &mut Session,
        args: &[String],
        _input: Value,
    ) -> Result<Value, StructuredDiagnostic> {
        let mut sig = Signal::SIGTERM;
        let mut pids = Vec::new();
        let mut iter = args.iter();

        while let Some(arg) = iter.next() {
            if arg == "-9" {
                sig = Signal::SIGKILL;
            } else if arg == "-15" {
                sig = Signal::SIGTERM;
            } else if arg == "-s"
                && let Some(s) = iter.next()
            {
                sig = parse_signal_name(s)?;
            } else if let Ok(p) = arg.parse::<i32>() {
                pids.push(Pid::from_raw(p));
            } else {
                return Err(StructuredDiagnostic::new(
                    "E_ARGS",
                    format!("invalid kill argument `{arg}`"),
                    None,
                    None,
                ));
            }
        }

        if pids.is_empty() {
            return Err(StructuredDiagnostic::new(
                "E_ARGS",
                "kill requires at least one PID",
                None,
                None,
            ));
        }

        for pid in pids {
            nix_kill(pid, sig)
                .map_err(|e| StructuredDiagnostic::new("E_KILL", e.to_string(), None, None))?;
        }
        Ok(Value::Empty)
    }
}

fn parse_signal_name(name: &str) -> Result<Signal, StructuredDiagnostic> {
    match name.to_uppercase().trim_start_matches("SIG") {
        "INT" => Ok(Signal::SIGINT),
        "TERM" => Ok(Signal::SIGTERM),
        "KILL" => Ok(Signal::SIGKILL),
        "HUP" => Ok(Signal::SIGHUP),
        "STOP" => Ok(Signal::SIGSTOP),
        "CONT" => Ok(Signal::SIGCONT),
        _ => Err(StructuredDiagnostic::new(
            "E_SIGNAL",
            format!("unknown signal `{name}`"),
            None,
            None,
        )),
    }
}
