use nix::sys::signal::{Signal, kill as nix_kill};
use nix::unistd::Pid;
use serde_json::{Value as JsonVal, json};
use shell_kernel::Session;
use shell_value::{StructuredDiagnostic, StructuredRecord, Value};

pub(crate) fn ps() -> Result<Value, StructuredDiagnostic> {
    let mut records = Vec::new();
    let entries = std::fs::read_dir("/proc")
        .map_err(|e| StructuredDiagnostic::from_io("E_PS", &e, None))?;

    for entry_res in entries {
        let Ok(entry) = entry_res else {
            continue;
        };
        let fname = entry.file_name().to_string_lossy().to_string();
        let Ok(pid) = fname.parse::<u32>() else {
            continue;
        };
        let stat_path = entry.path().join("stat");
        let Ok(stat_str) = std::fs::read_to_string(&stat_path) else {
            continue;
        };
        let (name, state) = parse_proc_stat(&stat_str);
        let cmdline = std::fs::read(entry.path().join("cmdline"))
            .map(|b| {
                b.split(|&c| c == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let mut rec = StructuredRecord::new();
        rec.insert("pid".to_string(), json!(pid));
        rec.insert("name".to_string(), JsonVal::String(name));
        rec.insert("state".to_string(), JsonVal::String(state));
        rec.insert("cmdline".to_string(), JsonVal::String(cmdline));
        records.push(rec);
    }

    records.sort_by_key(|r| r.get("pid").and_then(|v| v.as_u64()).unwrap_or(0));
    Ok(Value::from_records(records))
}

fn parse_proc_stat(stat: &str) -> (String, String) {
    let Some(l) = stat.find('(') else {
        return (String::new(), String::new());
    };
    let Some(r) = stat.rfind(')') else {
        return (String::new(), String::new());
    };
    let name = stat.get(l + 1..r).unwrap_or("").to_string();
    let state = stat.get(r + 2..r + 3).unwrap_or("").to_string();
    (name, state)
}

pub(crate) fn kill(args: &[String]) -> Result<Value, StructuredDiagnostic> {
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

pub(crate) fn which(session: &Session, args: &[String]) -> Result<Value, StructuredDiagnostic> {
    let path_var = session.env.get("PATH").unwrap_or_default();

    let mut records = Vec::new();
    for cmd in args {
        let mut found = None;
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                found = Some(candidate.to_string_lossy().to_string());
                break;
            }
        }
        if let Some(p) = found {
            let mut rec = StructuredRecord::new();
            rec.insert("name".to_string(), JsonVal::String(cmd.clone()));
            rec.insert("path".to_string(), JsonVal::String(p));
            records.push(rec);
        }
    }
    Ok(Value::from_records(records))
}
