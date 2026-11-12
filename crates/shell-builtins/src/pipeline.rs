use std::io::Write;
use std::process::Stdio;

use shell_kernel::{Session, execute_pipeline, expand_word, reap_background_jobs};
use shell_surface::{SurfaceMode, render_surface, render_surface_sync};
use shell_syntax::{Command, ListOp, Pipeline, Program, RedirectKind};
use shell_value::{StructuredDiagnostic, TaintLevel, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;

use crate::builtins::{is_async_builtin, run_builtin, run_builtin_sync};
use crate::schema::is_builtin;

pub fn program_needs_async(session: &Session, program: &Program) -> bool {
    program
        .items
        .iter()
        .any(|item| pipeline_needs_async(session, &item.pipeline))
}

pub fn pipeline_needs_async(session: &Session, pipeline: &Pipeline) -> bool {
    pipeline.commands.iter().any(|cmd| {
        if let Command::Simple(simple) = cmd
            && let Some(first) = simple.words.first()
        {
            let name = expand_word(first, &session.env, session.last_status);
            return is_async_builtin(&name);
        }
        false
    })
}

pub fn execute_program_surface_sync(
    session: &mut Session,
    program: &Program,
    mode: SurfaceMode,
) -> Result<i32, StructuredDiagnostic> {
    reap_background_jobs(session);

    let mut last_status = session.last_status;
    let mut prev_op: Option<ListOp> = None;

    for item in &program.items {
        if let Some(op) = prev_op {
            match op {
                ListOp::And if last_status != 0 => {
                    prev_op = item.op;
                    continue;
                }
                ListOp::Or if last_status == 0 => {
                    prev_op = item.op;
                    continue;
                }
                _ => {}
            }
        }

        let background = matches!(item.op, Some(ListOp::Background));
        if background || !pipeline_uses_builtin(session, &item.pipeline) {
            last_status = execute_pipeline(session, &item.pipeline, background)?;
        } else {
            let (val, code) = execute_hybrid_pipeline_sync(session, &item.pipeline)?;
            let mut stdout = std::io::stdout();
            render_surface_sync(val, &mut stdout, mode)?;
            last_status = code;
        }

        session.last_status = last_status;
        prev_op = item.op;
        if session.should_exit.is_some() {
            break;
        }
    }

    Ok(last_status)
}

pub async fn execute_program_tty(
    session: &mut Session,
    program: &Program,
    color: bool,
) -> Result<i32, StructuredDiagnostic> {
    let mode = SurfaceMode::detect(false, color);
    execute_program_surface(session, program, mode).await
}

pub async fn execute_program_surface(
    session: &mut Session,
    program: &Program,
    mode: SurfaceMode,
) -> Result<i32, StructuredDiagnostic> {
    reap_background_jobs(session);

    let mut last_status = session.last_status;
    let mut prev_op: Option<ListOp> = None;

    for item in &program.items {
        if let Some(op) = prev_op {
            match op {
                ListOp::And if last_status != 0 => {
                    prev_op = item.op;
                    continue;
                }
                ListOp::Or if last_status == 0 => {
                    prev_op = item.op;
                    continue;
                }
                _ => {}
            }
        }

        let background = matches!(item.op, Some(ListOp::Background));
        if background || !pipeline_uses_builtin(session, &item.pipeline) {
            last_status = execute_pipeline(session, &item.pipeline, background)?;
        } else {
            let (val, code) = execute_hybrid_pipeline(session, &item.pipeline).await?;
            let mut stdout = std::io::stdout();
            render_surface(val, &mut stdout, mode).await?;
            last_status = code;
        }

        session.last_status = last_status;
        prev_op = item.op;
        if session.should_exit.is_some() {
            break;
        }
    }

    Ok(last_status)
}

pub async fn execute_program_value(
    session: &mut Session,
    program: &Program,
) -> Result<(Value, i32), StructuredDiagnostic> {
    let mut last_val = Value::Empty;
    let mut last_status = session.last_status;
    let mut prev_op: Option<ListOp> = None;

    for item in &program.items {
        if let Some(op) = prev_op {
            match op {
                ListOp::And if last_status != 0 => {
                    prev_op = item.op;
                    continue;
                }
                ListOp::Or if last_status == 0 => {
                    prev_op = item.op;
                    continue;
                }
                _ => {}
            }
        }

        let (val, code) = execute_hybrid_pipeline(session, &item.pipeline).await?;
        last_val = val;
        last_status = code;
        session.last_status = last_status;
        prev_op = item.op;

        if session.should_exit.is_some() {
            break;
        }
    }

    Ok((last_val, last_status))
}

pub fn pipeline_uses_builtin(session: &Session, pipeline: &Pipeline) -> bool {
    pipeline.commands.iter().any(|cmd| {
        if let Command::Simple(simple) = cmd {
            if let Some(first) = simple.words.first() {
                let name = expand_word(first, &session.env, session.last_status);
                return is_builtin(&name) && name != "jobs" && name != "fg" && name != "bg" && name != "exit";
            }
        }
        false
    })
}

pub fn execute_hybrid_pipeline_sync(
    session: &mut Session,
    pipeline: &Pipeline,
) -> Result<(Value, i32), StructuredDiagnostic> {
    let mut current_val = Value::Empty;
    let mut last_code = 0;
    let total = pipeline.commands.len();

    begin_pipeline_tx(session, pipeline);

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        let is_last = idx + 1 == total;
        match cmd {
            Command::PosixFallback { raw, .. } => {
                let (out, code) = run_external_captured_sync(
                    session,
                    "sh",
                    &["-c".to_string(), raw.clone()],
                    &[],
                    current_val,
                )?;
                current_val = out;
                last_code = code;
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
                if is_builtin(name) && !matches!(name.as_str(), "jobs" | "fg" | "bg" | "exit") {
                    current_val = run_builtin_sync(session, name, &args[1..], current_val)?;
                    last_code = 0;

                    if is_last && !simple.redirections.is_empty() {
                        current_val = apply_value_redirections_sync(session, simple, current_val)?;
                    }
                } else if is_last && matches!(current_val, Value::Empty) && simple.redirections.is_empty() {
                    let single_pipe = Pipeline {
                        commands: vec![cmd.clone()],
                        span: cmd.span(),
                    };
                    let code = execute_pipeline(session, &single_pipe, false)?;
                    current_val = Value::Empty;
                    last_code = code;
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
                    let (out, code) = run_external_captured_sync(
                        session,
                        name,
                        &args[1..],
                        &env_pairs,
                        current_val,
                    )?;
                    current_val = out;
                    last_code = code;
                    if is_last && !simple.redirections.is_empty() {
                        current_val = apply_value_redirections_sync(session, simple, current_val)?;
                    }
                }
            }
        }
    }

    session.finish_tx();
    Ok((current_val, last_code))
}

fn begin_pipeline_tx(session: &mut Session, pipeline: &Pipeline) {
    let first_cmd_name = pipeline.commands.first().and_then(|c| match c {
        Command::Simple(s) => s
            .words
            .first()
            .map(|w| expand_word(w, &session.env, session.last_status)),
        _ => None,
    });
    if !matches!(
        first_cmd_name.as_deref(),
        Some("undo" | "rollback" | "tx-log")
    ) {
        let summary = first_cmd_name.unwrap_or_else(|| "pipeline".to_string());
        session.begin_tx(summary);
    }
}

pub async fn execute_hybrid_pipeline(
    session: &mut Session,
    pipeline: &Pipeline,
) -> Result<(Value, i32), StructuredDiagnostic> {
    if !pipeline_needs_async(session, pipeline) {
        return execute_hybrid_pipeline_sync(session, pipeline);
    }

    let mut current_val = Value::Empty;
    let mut last_code = 0;
    let total = pipeline.commands.len();

    begin_pipeline_tx(session, pipeline);

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        let is_last = idx + 1 == total;
        match cmd {
            Command::PosixFallback { raw, .. } => {
                let (out, code) = run_external_captured(
                    session,
                    "sh",
                    &["-c".to_string(), raw.clone()],
                    &[],
                    current_val,
                )
                .await?;
                current_val = out;
                last_code = code;
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
                if is_builtin(name) && !matches!(name.as_str(), "jobs" | "fg" | "bg" | "exit") {
                    current_val = run_builtin(session, name, &args[1..], current_val).await?;
                    last_code = 0;

                    if is_last && !simple.redirections.is_empty() {
                        current_val = apply_value_redirections(session, simple, current_val).await?;
                    }
                } else if is_last && matches!(current_val, Value::Empty) && simple.redirections.is_empty() {
                    let single_pipe = Pipeline {
                        commands: vec![cmd.clone()],
                        span: cmd.span(),
                    };
                    let code = execute_pipeline(session, &single_pipe, false)?;
                    current_val = Value::Empty;
                    last_code = code;
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
                    let (out, code) = run_external_captured(
                        session,
                        name,
                        &args[1..],
                        &env_pairs,
                        current_val,
                    )
                    .await?;
                    current_val = out;
                    last_code = code;
                    if is_last && !simple.redirections.is_empty() {
                        current_val = apply_value_redirections(session, simple, current_val).await?;
                    }
                }
            }
        }
    }

    session.finish_tx();
    Ok((current_val, last_code))
}

fn apply_value_redirections_sync(
    session: &mut Session,
    simple: &shell_syntax::SimpleCommand,
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
                    let bytes = current.into_raw_bytes_sync()?;
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

async fn apply_value_redirections(
    session: &mut Session,
    simple: &shell_syntax::SimpleCommand,
    val: Value,
) -> Result<Value, StructuredDiagnostic> {
    let mat = match val {
        Value::RawStream(_) => Value::Bytes(val.into_raw_bytes().await?),
        Value::RecordStream(_) => Value::Batch(val.into_batch().await?),
        other => other,
    };
    apply_value_redirections_sync(session, simple, mat)
}

fn run_external_captured_sync(
    session: &Session,
    prog: &str,
    args: &[String],
    extra_env: &[(String, String)],
    input: Value,
) -> Result<(Value, i32), StructuredDiagnostic> {
    let has_input = !matches!(input, Value::Empty);
    let input_bytes = if has_input {
        input.into_raw_bytes_sync()?
    } else {
        Vec::new()
    };

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
        StructuredDiagnostic::new("E_SPAWN", format!("{prog}: {e}"), None, None)
    })?;

    if has_input
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(&input_bytes);
    }

    let output = child
        .wait_with_output()
        .map_err(|e| StructuredDiagnostic::from_io("E_WAIT", &e, None))?;

    if !output.stderr.is_empty() {
        let _ = std::io::stderr().write_all(&output.stderr);
    }

    let code = output.status.code().unwrap_or(1);
    Ok((Value::from_bytes(output.stdout), code))
}

async fn run_external_captured(
    session: &Session,
    prog: &str,
    args: &[String],
    extra_env: &[(String, String)],
    input: Value,
) -> Result<(Value, i32), StructuredDiagnostic> {
    let has_input = !matches!(input, Value::Empty);
    let input_bytes = if has_input {
        input.into_raw_bytes().await?
    } else {
        Vec::new()
    };

    let mut command = TokioCommand::new(prog);
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
        StructuredDiagnostic::new("E_SPAWN", format!("{prog}: {e}"), None, None)
    })?;

    if has_input
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(&input_bytes).await;
    }

    let output = child.wait_with_output().await.map_err(|e| {
        StructuredDiagnostic::from_io("E_WAIT", &e, None)
    })?;

    if !output.stderr.is_empty() {
        let _ = std::io::stderr().write_all(&output.stderr);
    }

    let code = output.status.code().unwrap_or(1);
    Ok((Value::from_bytes(output.stdout), code))
}
