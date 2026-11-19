use std::io::Write;

use nix::unistd::Pid;
use shell_kernel::{
    JobState, Session, command_word, expand_word, expand_words, find_executable,
    reap_background_jobs,
};
use shell_surface::SurfaceMode;
use shell_syntax::{Command, ListOp, Pipeline, Program, RedirectKind, Redirection, SimpleCommand};
use shell_value::{StructuredDiagnostic, TaintLevel, Value};

use crate::args::Args;
use crate::builtin::{Builtin, Placement, TxPolicy};
use crate::builtins::lookup;
use crate::env_capture::EnvCapture;
use crate::sink::{Sink, diag_status};
use crate::wiring::{
    Flow, LaunchError, Launched, Plan, finish_foreground, launch_external, read_all,
    read_all_watching, read_redirect_file, resolve_redirs,
};

pub const KILLED_STATUS: i32 = 124;

const NETWORK_COMMANDS: [&str; 6] = ["curl", "wget", "nc", "ssh", "scp", "rsync"];

pub(crate) enum StageKind {
    Assign,
    Builtin(&'static dyn Builtin),
    External,
}

pub(crate) struct Stage<'a> {
    pub kind: StageKind,
    pub label: String,
    pub argv: Vec<String>,
    pub display: Option<String>,
    pub env: Vec<(String, String)>,
    pub redirs: &'a [Redirection],
    pub read_only: bool,
}

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
            if session.kill.is_killed() {
                session.last_status = KILLED_STATUS;
                return KILLED_STATUS;
            }
            let background = matches!(item.op, Some(ListOp::Background));
            status = run_pipeline(session, &item.pipeline, background, &program.source, sink);
            session.last_status = status;
            if session.should_exit.is_some() {
                break;
            }
        }
        prev_op = item.op;
    }

    status
}

fn run_pipeline(
    session: &mut Session,
    pipeline: &Pipeline,
    background: bool,
    source: &str,
    sink: &Sink,
) -> i32 {
    if pipeline.commands.is_empty() {
        return 0;
    }
    if !background
        && let [Command::Simple(simple)] = pipeline.commands.as_slice()
        && let Some(status) = run_alone(session, simple, sink)
    {
        return status;
    }

    begin_pipeline_tx(session, pipeline);
    let capture = match pipeline.commands.as_slice() {
        [Command::PosixFallback { .. }] if !background => EnvCapture::create(session),
        _ => None,
    };
    let mut stages = build_stages(session, pipeline, sink, capture.as_ref());
    if background
        && stages
            .iter()
            .any(|s| matches!(s.kind, StageKind::Builtin(_)))
    {
        match detached_stage(session, pipeline, source, sink) {
            Ok(stage) => stages = vec![stage],
            Err(diag) => {
                sink.diag(&diag);
                session.finish_tx();
                return 1;
            }
        }
    }
    let (status, tracked) = std::thread::scope(|scope| {
        let plan = Plan {
            sink,
            scope,
            background,
        };
        run_stages(session, &stages, &plan)
    });
    if let Some(pgid) = tracked {
        session.kill.untrack(pgid);
    }
    session.finish_tx();
    if let Some(capture) = capture {
        capture.apply(session);
    }
    status
}

fn detached_stage<'a>(
    session: &Session,
    pipeline: &Pipeline,
    source: &str,
    sink: &Sink,
) -> Result<Stage<'a>, StructuredDiagnostic> {
    let exe = std::env::current_exe()
        .map_err(|e| StructuredDiagnostic::from_io("E_SPAWN", &e, Some(pipeline.span)))?;
    let text = pipeline.span.slice(source).to_string();
    let mut argv = vec![exe.to_string_lossy().into_owned()];
    if matches!(
        sink,
        Sink::Tty {
            mode: SurfaceMode::TernSurfaceProtocol
        }
    ) {
        argv.push("--tsp".to_string());
    }
    argv.push("--last-status".to_string());
    argv.push(session.last_status.to_string());
    if let Some(pid) = session.env.get("!") {
        argv.push("--last-bg".to_string());
        argv.push(pid);
    }
    argv.push("-c".to_string());
    argv.push(text.clone());
    Ok(Stage {
        kind: StageKind::External,
        label: text.clone(),
        argv,
        display: Some(text),
        env: Vec::new(),
        redirs: &[],
    })
}

fn command_name(session: &Session, cmd: &Command) -> Option<String> {
    match cmd {
        Command::Simple(simple) => command_word(&simple.words, session),
        Command::PosixFallback { .. } => None,
    }
}

fn skips_tx(name: &str) -> bool {
    lookup(name).is_some_and(|b| {
        let sig = b.signature();
        sig.tx == TxPolicy::Skip && sig.name == name
    })
}

fn stage_builtin(name: &str) -> Option<&'static dyn Builtin> {
    lookup(name).filter(|b| b.signature().placement == Placement::Any)
}

fn defers_to_executable(
    session: &mut Session,
    builtin: &dyn Builtin,
    argv: &[String],
    sink: &Sink,
) -> bool {
    let sig = builtin.signature();
    if !sig.strict_options {
        return false;
    }
    let Some(option) = Args::unknown_option(sig, &argv[1..]) else {
        return false;
    };
    let Some(path) = find_executable(session, &argv[0]) else {
        return false;
    };
    if session.first_fallback(&argv[0]) {
        sink.stderr_line(&format!(
            "koa: {}: '{option}' is not built in, running {}",
            argv[0],
            path.display()
        ));
    }
    true
}

fn run_alone(session: &mut Session, simple: &SimpleCommand, sink: &Sink) -> Option<i32> {
    let builtin = lookup(&command_word(&simple.words, session)?)?;
    if builtin.signature().placement != Placement::SoleStage {
        return None;
    }
    let args = expand_words(&simple.words, session);
    Some(builtin.run_alone(session, args.get(1..)?, sink))
}

fn begin_pipeline_tx(session: &mut Session, pipeline: &Pipeline) {
    let first_cmd_name = pipeline
        .commands
        .first()
        .and_then(|c| command_name(session, c));
    if !first_cmd_name.as_deref().is_some_and(skips_tx) {
        let summary = first_cmd_name.unwrap_or_else(|| "pipeline".to_string());
        session.begin_tx(summary);
    }
}

fn build_stages<'a>(
    session: &mut Session,
    pipeline: &'a Pipeline,
    sink: &Sink,
    capture: Option<&EnvCapture>,
) -> Vec<Stage<'a>> {
    let mut stages = Vec::with_capacity(pipeline.commands.len());
    for cmd in &pipeline.commands {
        let stage = match cmd {
            Command::PosixFallback { raw, .. } => {
                let shell = if find_executable(session, "bash").is_some() {
                    "bash"
                } else {
                    "sh"
                };
                Stage {
                    kind: StageKind::External,
                    label: raw.clone(),
                    argv: match capture {
                        Some(capture) => capture.argv(shell, raw),
                        None => vec![shell.to_string(), "-c".to_string(), raw.clone()],
                    },
                    display: Some(format!("{shell} -c {raw}")),
                    env: capture.map(EnvCapture::env).unwrap_or_default(),
                    redirs: &[],
                    read_only: false,
                }
            }
            Command::Simple(simple) => {
                let argv = expand_words(&simple.words, session);
                let env: Vec<(String, String)> = simple
                    .assignments
                    .iter()
                    .map(|a| {
                        (
                            a.name.clone(),
                            expand_word(&a.value, &session.env, session.last_status),
                        )
                    })
                    .collect();
                let view = ls_view(session, pipeline, simple, &argv, sink);
                let read_only = view.is_some();
                let argv = view.unwrap_or(argv);
                let builtin = argv
                    .first()
                    .and_then(|name| stage_builtin(name))
                    .filter(|b| !defers_to_executable(session, *b, &argv, sink));
                match (argv.first(), builtin) {
                    (None, _) => {
                        for (name, value) in env {
                            session.env.insert(name, value);
                        }
                        Stage {
                            kind: StageKind::Assign,
                            label: String::new(),
                            argv,
                            display: None,
                            env: Vec::new(),
                            redirs: &simple.redirections,
                            read_only: false,
                        }
                    }
                    (Some(name), Some(builtin)) => Stage {
                        kind: StageKind::Builtin(builtin),
                        label: name.clone(),
                        argv,
                        display: None,
                        env,
                        redirs: &simple.redirections,
                        read_only: false,
                    },
                    (Some(name), None) => {
                        if NETWORK_COMMANDS.contains(&name.as_str()) {
                            session.taint = TaintLevel::UntrustedExternal;
                        }
                        Stage {
                            kind: StageKind::External,
                            label: name.clone(),
                            argv,
                            display: None,
                            env,
                            redirs: &simple.redirections,
                            read_only,
                        }
                    }
                }
            }
        };
        stages.push(stage);
    }
    stages
}

const DEFAULT_LS_VIEW: &str = "eza -lh --group-directories-first --icons=auto";

fn ls_view(
    session: &Session,
    pipeline: &Pipeline,
    simple: &SimpleCommand,
    argv: &[String],
    sink: &Sink,
) -> Option<Vec<String>> {
    let (name, operands) = argv.split_first()?;
    let bare = name == "ls"
        && pipeline.commands.len() == 1
        && simple.redirections.is_empty()
        && !sink.is_capture()
        && !operands.iter().any(|a| a.starts_with('-'))
        && nix::unistd::isatty(1).unwrap_or(false);
    if !bare {
        return None;
    }
    let view = session
        .env
        .get("KOA_LS_VIEW")
        .unwrap_or_else(|| DEFAULT_LS_VIEW.to_string());
    let mut view: Vec<String> = view.split_whitespace().map(str::to_string).collect();
    find_executable(session, view.first()?)?;
    view.extend(operands.iter().cloned());
    Some(view)
}

fn summarize(stages: &[Stage]) -> String {
    stages
        .iter()
        .map(|s| s.display.clone().unwrap_or_else(|| s.argv.join(" ")))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn run_stages(session: &mut Session, stages: &[Stage], plan: &Plan) -> (i32, Option<Pid>) {
    let mut flow = Flow::None;
    let mut launched = Launched::default();
    let mut final_status: Option<i32> = None;
    let mut last_is_launched = false;

    for (idx, stage) in stages.iter().enumerate() {
        let last = idx + 1 == stages.len();
        if session.kill.is_killed() {
            final_status = Some(KILLED_STATUS);
            break;
        }
        last_is_launched = false;
        match stage.kind {
            StageKind::Assign => {
                flow = Flow::Value(Value::Empty);
                final_status = Some(match resolve_redirs(session, stage.redirs) {
                    Ok(_) => 0,
                    Err(diag) => {
                        plan.sink.diag(&diag);
                        1
                    }
                });
            }
            StageKind::Builtin(builtin) => {
                let input = take_builtin_input(session, builtin, stage, &mut flow, &launched, plan);
                let result = input.and_then(|value| builtin.run(session, &stage.argv[1..], value));
                let reports = session.take_reports();
                for report in &reports {
                    plan.sink.diag(report);
                }
                let soft_failure = i32::from(!reports.is_empty());
                let status = match result {
                    Ok(value) if last => match finish_builtin_output(session, stage, value) {
                        Ok(value) => {
                            let columns = stage
                                .env
                                .iter()
                                .rev()
                                .find(|(name, _)| name == "COLUMNS")
                                .map(|(_, value)| value.clone())
                                .or_else(|| session.env.get("COLUMNS"));
                            plan.sink.emit(value, columns.as_deref());
                            soft_failure
                        }
                        Err(diag) => {
                            plan.sink.diag(&diag);
                            diag_status(&diag)
                        }
                    },
                    Ok(value) => {
                        flow = Flow::Value(value);
                        soft_failure
                    }
                    Err(diag) => {
                        plan.sink.diag(&diag);
                        flow = Flow::Value(Value::Empty);
                        diag_status(&diag)
                    }
                };
                final_status = Some(status);
            }
            StageKind::External => {
                match launch_external(session, stage, &mut flow, plan, last, &mut launched) {
                    Ok(()) => {
                        last_is_launched = true;
                        final_status = None;
                    }
                    Err(LaunchError { status, diag }) => {
                        plan.sink.diag(&diag);
                        flow = Flow::Value(Value::Empty);
                        final_status = Some(status);
                    }
                }
            }
        }
    }
    drop(flow);

    let tracked = launched.tracked;
    let summary = summarize(stages);
    let Some(pgid) = launched.pgid else {
        return (final_status.unwrap_or(0), tracked);
    };
    if plan.background {
        let last = launched.last_pid.unwrap_or(pgid);
        session.env.set_last_background(last.as_raw());
        let id = session.add_job(
            pgid,
            launched.pids,
            launched.last_pid,
            None,
            summary,
            JobState::Running,
        );
        plan.sink.stderr_line(&format!("[{id}] {last}"));
        return (final_status.unwrap_or(0), tracked);
    }
    let watch = last_is_launched.then_some(launched.last_pid).flatten();
    let waited = finish_foreground(
        session,
        plan.sink,
        pgid,
        &launched.pids,
        watch,
        &summary,
        None,
    );
    (final_status.unwrap_or(waited), tracked)
}

fn take_builtin_input(
    session: &Session,
    builtin: &dyn Builtin,
    stage: &Stage,
    flow: &mut Flow,
    launched: &Launched,
    plan: &Plan,
) -> Result<Value, StructuredDiagnostic> {
    let from_file = stage
        .redirs
        .iter()
        .rev()
        .find(|r| r.kind == RedirectKind::Stdin);
    let flowed = std::mem::replace(flow, Flow::None);
    if let Some(redir) = from_file {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut read_redirect_file(session, redir)?, &mut bytes)
            .map_err(|e| StructuredDiagnostic::from_io("E_REDIR", &e, Some(redir.span)))?;
        return Ok(Value::Bytes(bytes));
    }
    match flowed {
        Flow::None if reads_shell_stdin(builtin, stage, plan) => {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin().lock(), &mut bytes)
                .map_err(|e| StructuredDiagnostic::from_io("E_STDIN", &e, None))?;
            Ok(Value::Bytes(bytes))
        }
        Flow::None => Ok(Value::Empty),
        Flow::Value(value) => Ok(value),
        Flow::Pipe(fd) => match launched.pgid {
            Some(producers) if session.interactive => read_all_watching(plan, fd, producers),
            _ => read_all(fd),
        }
        .map(Value::Bytes)
        .map_err(|e| StructuredDiagnostic::from_io("E_PIPE", &e, None)),
    }
}

fn reads_shell_stdin(builtin: &dyn Builtin, stage: &Stage, plan: &Plan) -> bool {
    !plan.sink.is_capture()
        && !nix::unistd::isatty(0).unwrap_or(true)
        && builtin.reads_input(&stage.argv[1..])
}

fn finish_builtin_output(
    session: &mut Session,
    stage: &Stage,
    value: Value,
) -> Result<Value, StructuredDiagnostic> {
    let mut current = value;
    for redir in stage.redirs {
        if !matches!(
            redir.kind,
            RedirectKind::StdoutTrunc | RedirectKind::StdoutAppend
        ) {
            continue;
        }
        let Some(target) = &redir.target else {
            continue;
        };
        let raw = expand_word(target, &session.env, session.last_status);
        let path = session.cwd.join(raw);
        let bytes = current.into_raw_bytes();
        session.record_output_target(&path);
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
    Ok(current)
}
