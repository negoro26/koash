use std::io::Write;

use nix::unistd::Pid;
use shell_kernel::{JobState, Session, expand_word, reap_background_jobs};
use shell_syntax::{Command, ListOp, Pipeline, Program, RedirectKind, Redirection, SimpleCommand};
use shell_value::{StructuredDiagnostic, TaintLevel, Value};

use crate::builtin::{Builtin, Placement, TxPolicy};
use crate::builtins::lookup;
use crate::sink::{Sink, diag_status};
use crate::wiring::{
    Flow, LaunchError, Launched, Plan, finish_foreground, launch_external, read_all,
    read_all_watching, read_redirect_file,
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
    if let [Command::PosixFallback { raw, .. }] = pipeline.commands.as_slice() {
        let status = crate::embedded::run_embedded(session, raw, sink);
        session.finish_tx();
        return status;
    }
    let stages = build_stages(session, pipeline);
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
    status
}

fn command_name(session: &Session, cmd: &Command) -> Option<String> {
    match cmd {
        Command::Simple(simple) => simple
            .words
            .first()
            .map(|w| expand_word(w, &session.env, session.last_status)),
        Command::PosixFallback { raw, .. } => Some(raw.clone()),
    }
}

fn skips_tx(name: &str) -> bool {
    // `tx_log`, the alias of `tx-log`, has always opened its own ledger entry.
    lookup(name).is_some_and(|b| {
        let sig = b.signature();
        sig.tx == TxPolicy::Skip && sig.name == name
    })
}

fn stage_builtin(name: &str) -> Option<&'static dyn Builtin> {
    lookup(name).filter(|b| b.signature().placement == Placement::Any)
}

fn run_alone(session: &mut Session, simple: &SimpleCommand, sink: &Sink) -> Option<i32> {
    let args: Vec<String> = simple
        .words
        .iter()
        .map(|w| expand_word(w, &session.env, session.last_status))
        .collect();
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

fn build_stages<'a>(session: &mut Session, pipeline: &'a Pipeline) -> Vec<Stage<'a>> {
    let mut stages = Vec::with_capacity(pipeline.commands.len());
    for cmd in &pipeline.commands {
        let stage = match cmd {
            Command::PosixFallback { raw, .. } => Stage {
                kind: StageKind::External,
                label: raw.clone(),
                argv: vec!["sh".to_string(), "-c".to_string(), raw.clone()],
                env: Vec::new(),
                redirs: &[],
            },
            Command::Simple(simple) => {
                let argv: Vec<String> = simple
                    .words
                    .iter()
                    .map(|w| expand_word(w, &session.env, session.last_status))
                    .collect();
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
                let builtin = argv.first().and_then(|name| stage_builtin(name));
                match (argv.first(), builtin) {
                    (None, _) => {
                        for (name, value) in env {
                            session.env.insert(name, value);
                        }
                        Stage {
                            kind: StageKind::Assign,
                            label: String::new(),
                            argv,
                            env: Vec::new(),
                            redirs: &[],
                        }
                    }
                    (Some(name), Some(builtin)) => Stage {
                        kind: StageKind::Builtin(builtin),
                        label: name.clone(),
                        argv,
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
        .map(|s| s.argv.join(" "))
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
                final_status = Some(0);
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
