use std::io::Write;
use std::process::Stdio;

use shell_kernel::{Session, execute_pipeline, expand_word, reap_background_jobs};
use shell_syntax::{Command, ListOp, Pipeline, Program, RedirectKind, SimpleCommand};
use shell_value::{StructuredDiagnostic, TaintLevel, Value};

use crate::builtins::run_builtin;
use crate::schema::is_builtin;
use crate::sink::{Sink, diag_status};

const KERNEL_COMMANDS: [&str; 5] = ["jobs", "fg", "bg", "exit", "unset"];

/// Runs a program and returns the status of its last executed list item.
/// Builtin failures become a status; only a parse error (which never reaches here) aborts a program.
pub fn run_program(session: &mut Session, program: &Program, sink: &Sink) -> i32 {
    for notice in reap_background_jobs(session) {
        sink.stderr_line(&notice);
    }

    let mut status = session.last_status;
    let mut prev_op: Option<ListOp> = None;

    for item in &program.items {
        let skip = match prev_op {
            Some(ListOp::And) => status != 0,
            Some(ListOp::Or) => status == 0,
            _ => false,
        };
        if !skip {
            let background = matches!(item.op, Some(ListOp::Background));
            status = run_pipeline(session, &item.pipeline, background, sink);
            session.last_status = status;
            if session.should_exit.is_some() {
                break;
            }
        }
        prev_op = item.op;
    }

    status
}

fn run_pipeline(session: &mut Session, pipeline: &Pipeline, background: bool, sink: &Sink) -> i32 {
    if uses_fork_path(session, pipeline, background, sink) {
        return match execute_pipeline(session, pipeline, background) {
            Ok(code) => code,
            Err(diag) => {
                sink.diag(&diag);
                diag_status(&diag)
            }
        };
    }
    begin_pipeline_tx(session, pipeline);
    let status = run_stages(session, pipeline, sink);
    session.finish_tx();
    status
}

fn command_name(session: &Session, cmd: &Command) -> Option<String> {
    match cmd {
        Command::Simple(simple) => simple
            .words
            .first()
            .map(|w| expand_word(w, &session.env, session.last_status)),
        Command::PosixFallback { .. } => None,
    }
}

fn uses_fork_path(session: &Session, pipeline: &Pipeline, background: bool, sink: &Sink) -> bool {
    if sink.is_capture() {
        return false;
    }
    if background {
        return true;
    }
    if pipeline.commands.len() == 1
        && command_name(session, &pipeline.commands[0])
            .is_some_and(|name| KERNEL_COMMANDS.contains(&name.as_str()))
    {
        return true;
    }
    !pipeline
        .commands
        .iter()
        .any(|cmd| command_name(session, cmd).is_some_and(|name| is_builtin(&name)))
}

fn begin_pipeline_tx(session: &mut Session, pipeline: &Pipeline) {
    let first_cmd_name = pipeline
        .commands
        .first()
        .and_then(|c| command_name(session, c));
    if !matches!(
        first_cmd_name.as_deref(),
        Some("undo" | "rollback" | "tx-log")
    ) {
        let summary = first_cmd_name.unwrap_or_else(|| "pipeline".to_string());
        session.begin_tx(summary);
    }
}

fn run_stages(session: &mut Session, pipeline: &Pipeline, sink: &Sink) -> i32 {
    let mut current_val = Value::Empty;
    let mut last_code = 0;
    let total = pipeline.commands.len();

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        let is_last = idx + 1 == total;
        match cmd {
            Command::PosixFallback { raw, .. } => {
                session.mark_opaque(raw.clone());
                match run_external_captured(
                    session,
                    "sh",
                    &["-c".to_string(), raw.clone()],
                    &[],
                    current_val,
                    sink,
                ) {
                    Ok((out, code)) => {
                        current_val = out;
                        last_code = code;
                    }
                    Err((diag, code)) => {
                        sink.diag(&diag);
                        current_val = Value::Empty;
                        last_code = code;
                    }
                }
            }
            Command::Simple(simple) => {
                let args: Vec<String> = simple
                    .words
                    .iter()
                    .map(|w| expand_word(w, &session.env, session.last_status))
                    .collect();

                if args.is_empty() {
                    for assign in &simple.assignments {
                        let val = expand_word(&assign.value, &session.env, session.last_status);
                        session.env.insert(assign.name.clone(), val);
                    }
                    current_val = Value::Empty;
                    last_code = 0;
                    continue;
                }

                let name = &args[0];
                if is_builtin(name) {
                    match run_builtin(session, name, &args[1..], current_val) {
                        Ok(val) => {
                            current_val = val;
                            last_code = 0;
                            if is_last && !simple.redirections.is_empty() {
                                match apply_value_redirections(session, simple, current_val) {
                                    Ok(v) => current_val = v,
                                    Err(diag) => {
                                        sink.diag(&diag);
                                        current_val = Value::Empty;
                                        last_code = diag_status(&diag);
                                    }
                                }
                            }
                        }
                        Err(diag) => {
                            sink.diag(&diag);
                            current_val = Value::Empty;
                            last_code = diag_status(&diag);
                        }
                    }
                } else {
                    let mut env_pairs = Vec::new();
                    for assign in &simple.assignments {
                        let val = expand_word(&assign.value, &session.env, session.last_status);
                        env_pairs.push((assign.name.clone(), val));
                    }
                    if matches!(
                        name.as_str(),
                        "curl" | "wget" | "nc" | "ssh" | "scp" | "rsync"
                    ) {
                        session.taint = TaintLevel::UntrustedExternal;
                    }
                    session.mark_opaque(name.clone());
                    match run_external_captured(
                        session,
                        name,
                        &args[1..],
                        &env_pairs,
                        current_val,
                        sink,
                    ) {
                        Ok((out, code)) => {
                            current_val = out;
                            last_code = code;
                            if is_last && !simple.redirections.is_empty() {
                                match apply_value_redirections(session, simple, current_val) {
                                    Ok(v) => current_val = v,
                                    Err(diag) => {
                                        sink.diag(&diag);
                                        current_val = Value::Empty;
                                        last_code = diag_status(&diag);
                                    }
                                }
                            }
                        }
                        Err((diag, code)) => {
                            sink.diag(&diag);
                            current_val = Value::Empty;
                            last_code = code;
                        }
                    }
                }
            }
        }
    }

    sink.emit(current_val);
    last_code
}

fn apply_value_redirections(
    session: &mut Session,
    simple: &SimpleCommand,
    val: Value,
) -> Result<Value, StructuredDiagnostic> {
    let mut current = val;
    for redir in &simple.redirections {
        match redir.kind {
            RedirectKind::StdoutTrunc | RedirectKind::StdoutAppend => {
                if let Some(ref target_word) = redir.target {
                    let path_str = expand_word(target_word, &session.env, session.last_status);
                    let path = if std::path::Path::new(&path_str).is_absolute() {
                        std::path::PathBuf::from(&path_str)
                    } else {
                        session.cwd.join(&path_str)
                    };
                    let bytes = current.into_raw_bytes();
                    session.record_preimage(&path);
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(redir.kind == RedirectKind::StdoutTrunc)
                        .append(redir.kind == RedirectKind::StdoutAppend)
                        .open(&path)
                        .map_err(|e| StructuredDiagnostic::from_io("E_REDIR", &e, Some(redir.span)))?;
                    file.write_all(&bytes)
                        .map_err(|e| StructuredDiagnostic::from_io("E_REDIR", &e, Some(redir.span)))?;
                    session.record_mutation(path.to_string_lossy().to_string());
                    current = Value::Empty;
                }
            }
            _ => {}
        }
    }
    Ok(current)
}

fn run_external_captured(
    session: &Session,
    prog: &str,
    args: &[String],
    extra_env: &[(String, String)],
    input: Value,
    sink: &Sink,
) -> Result<(Value, i32), (StructuredDiagnostic, i32)> {
    let has_input = !matches!(input, Value::Empty);
    let input_bytes = input.into_raw_bytes();

    let mut command = std::process::Command::new(prog);
    command.args(args);
    command.current_dir(&session.cwd);
    command.env_clear();
    command.envs(&session.env.materialize());
    for (k, v) in extra_env {
        command.env(k, v);
    }
    command.stdin(if has_input { Stdio::piped() } else { Stdio::null() });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        let status = match e.kind() {
            std::io::ErrorKind::PermissionDenied => 126,
            _ => 127,
        };
        (
            StructuredDiagnostic::new("E_SPAWN", format!("{prog}: {e}"), None, None),
            status,
        )
    })?;

    if has_input
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(&input_bytes);
    }

    let output = child.wait_with_output().map_err(|e| {
        (StructuredDiagnostic::from_io("E_WAIT", &e, None), 1)
    })?;

    if !output.stderr.is_empty() {
        sink.stderr(output.stderr);
    }

    let code = output.status.code().unwrap_or(1);
    Ok((Value::from_bytes(output.stdout), code))
}
