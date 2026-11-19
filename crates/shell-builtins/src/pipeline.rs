use std::io::Write;

use nix::unistd::Pid;
use shell_kernel::{
    JobState, Session, expand_word, expand_words, find_executable, reap_background_jobs,
};
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
    let stages = build_stages(session, pipeline, sink, capture.as_ref());
    let status = if background
        && stages
            .iter()
            .any(|s| matches!(s.kind, StageKind::Builtin(_)))
    {
        sink.diag(&StructuredDiagnostic::new(
            "E_BG_BUILTIN",
            "builtins cannot run in the background yet",
            Some(pipeline.span),
            None,
        ));
        2
    } else {
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
        status
    };
    session.finish_tx();
    if let Some(capture) = capture {
        capture.apply(session);
    }
    status
}

fn command_name(session: &Session, cmd: &Command) -> Option<String> {
    match cmd {
        Command::Simple(simple) => expand_words(&simple.words, &session.env, session.last_status)
            .into_iter()
            .next(),
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
    let args = expand_words(&simple.words, &session.env, session.last_status);
    let builtin = lookup(args.first()?)?;
    (builtin.signature().placement == Placement::SoleStage)
        .then(|| builtin.run_alone(session, &args[1..], sink))
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
                }
            }
            Command::Simple(simple) => {
                let argv = expand_words(&simple.words, &session.env, session.last_status);
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
                        }
                    }
                    (Some(name), Some(builtin)) => Stage {
                        kind: StageKind::Builtin(builtin),
                        label: name.clone(),
                        argv,
                        display: None,
                        env: Vec::new(),
                        redirs: &simple.redirections,
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
                        }
                    }
                }
            }
        };
        stages.push(stage);
    }
    stages
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
                let viewed = (last && idx == 0 && shows_terminal_view(stage, plan))
                    .then(|| builtin.terminal_view(session, &stage.argv[1..]))
                    .flatten();
                let result = match viewed {
                    Some(view) => view.map(Value::Bytes),
                    None => {
                        let input =
                            take_builtin_input(session, builtin, stage, &mut flow, &launched, plan);
                        input.and_then(|value| builtin.run(session, &stage.argv[1..], value))
                    }
                };
                let reports = session.take_reports();
                for report in &reports {
                    plan.sink.diag(report);
                }
                let soft_failure = i32::from(!reports.is_empty());
                let status = match result {
                    Ok(value) if last => match finish_builtin_output(session, stage, value) {
                        Ok(value) => {
                            plan.sink.emit(value);
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
        let id = session.add_job(pgid, launched.pids, summary, JobState::Running);
        plan.sink.stderr_line(&format!("[{id}] {pgid}"));
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

fn shows_terminal_view(stage: &Stage, plan: &Plan) -> bool {
    plan.sink.shows_terminal_view()
        && !stage.redirs.iter().any(|r| {
            matches!(
                r.kind,
                RedirectKind::StdoutTrunc | RedirectKind::StdoutAppend
            )
        })
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
