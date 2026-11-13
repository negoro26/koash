use std::ffi::CString;
use std::os::fd::{AsRawFd, OwnedFd};

use nix::fcntl::{OFlag, open};
use nix::sys::signal::{SigHandler, Signal, killpg, signal};
use nix::sys::stat::Mode;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, dup2, execvpe, fork, getpid, pipe2, setpgid, tcsetpgrp};
use shell_syntax::{Command, ListOp, Pipeline, Program, RedirectKind, Redirection, SimpleCommand};
use shell_value::StructuredDiagnostic;

use crate::session::{JobState, Session};
use crate::word_expand::expand_word;

pub fn execute_program(session: &mut Session, program: &Program) -> Result<i32, StructuredDiagnostic> {
    reap_background_jobs(session);

    let mut last_status = session.last_status;
    let mut skip_next = false;
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

        if skip_next {
            skip_next = false;
            prev_op = item.op;
            continue;
        }

        let background = matches!(item.op, Some(ListOp::Background));
        last_status = execute_pipeline(session, &item.pipeline, background)?;
        session.last_status = last_status;
        prev_op = item.op;

        if session.should_exit.is_some() {
            break;
        }
    }

    Ok(last_status)
}

pub fn execute_pipeline(
    session: &mut Session,
    pipeline: &Pipeline,
    background: bool,
) -> Result<i32, StructuredDiagnostic> {
    if pipeline.commands.is_empty() {
        return Ok(0);
    }

    if !background
        && pipeline.commands.len() == 1
        && let Command::Simple(cmd) = &pipeline.commands[0]
        && let Some(status) = try_run_kernel_builtin(session, cmd)?
    {
        return Ok(status);
    }

    let launched = match &pipeline.commands[0] {
        Command::Simple(s) => s
            .words
            .first()
            .map(|w| expand_word(w, &session.env, session.last_status))
            .unwrap_or_default(),
        Command::PosixFallback { raw, .. } => raw.clone(),
    };
    session.mark_opaque(launched);

    let num_cmds = pipeline.commands.len();
    let mut prev_read: Option<OwnedFd> = None;
    let mut pgid: Option<Pid> = None;
    let mut pids: Vec<Pid> = Vec::with_capacity(num_cmds);

    for (idx, cmd) in pipeline.commands.iter().enumerate() {
        let is_last = idx + 1 == num_cmds;
        let (next_read, next_write) = if !is_last {
            let (r, w) = pipe2(OFlag::O_CLOEXEC)
                .map_err(|e| StructuredDiagnostic::new("E_PIPE", e.to_string(), Some(cmd.span()), None))?;
            (Some(r), Some(w))
        } else {
            (None, None)
        };

        let fork_res = unsafe { fork() }.map_err(|e| {
            StructuredDiagnostic::new("E_FORK", e.to_string(), Some(cmd.span()), None)
        })?;

        match fork_res {
            ForkResult::Child => {
                let my_pid = getpid();
                let target_pgid = pgid.unwrap_or(my_pid);
                let _ = setpgid(my_pid, target_pgid);

                if !background && session.interactive {
                    let stdin = std::io::stdin();
                    let _ = tcsetpgrp(&stdin, target_pgid);
                }

                unsafe {
                    let _ = signal(Signal::SIGINT, SigHandler::SigDfl);
                    let _ = signal(Signal::SIGQUIT, SigHandler::SigDfl);
                    let _ = signal(Signal::SIGTSTP, SigHandler::SigDfl);
                    let _ = signal(Signal::SIGTTIN, SigHandler::SigDfl);
                    let _ = signal(Signal::SIGTTOU, SigHandler::SigDfl);
                }

                if let Some(ref rfd) = prev_read {
                    let raw = rfd.as_raw_fd();
                    if raw != 0 {
                        let _ = dup2(raw, 0);
                    }
                }
                if let Some(ref wfd) = next_write {
                    let raw = wfd.as_raw_fd();
                    if raw != 1 {
                        let _ = dup2(raw, 1);
                    }
                }

                drop(prev_read);
                drop(next_read);
                drop(next_write);

                exec_command_in_child(session, cmd);
            }
            ForkResult::Parent { child } => {
                let target_pgid = pgid.unwrap_or(child);
                if pgid.is_none() {
                    pgid = Some(target_pgid);
                }
                let _ = setpgid(child, target_pgid);
                if !background && session.interactive && pids.is_empty() {
                    let stdin = std::io::stdin();
                    let _ = tcsetpgrp(&stdin, target_pgid);
                }
                pids.push(child);

                drop(prev_read);
                drop(next_write);
                prev_read = next_read;
            }
        }
    }

    let Some(job_pgid) = pgid else {
        return Ok(0);
    };

    let cmd_summary = summarize_pipeline(session, pipeline);

    if background {
        let job_id = session.add_job(job_pgid, pids, cmd_summary, JobState::Running);
        eprintln!("[{job_id}] {job_pgid}");
        Ok(0)
    } else {
        wait_for_foreground_job(session, job_pgid, pids, cmd_summary, None)
    }
}

fn build_c_env(session: &Session, extra: &[(String, String)]) -> Vec<CString> {
    let mut env = session.env.materialize();
    for (k, v) in extra {
        env.insert(k.clone(), v.clone());
    }
    env.into_iter()
        .filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok())
        .collect()
}

fn exec_command_in_child(session: &Session, cmd: &Command) -> ! {
    let _ = nix::unistd::chdir(&session.cwd);
    match cmd {
        Command::PosixFallback { raw, .. } => {
            let Ok(sh) = CString::new("sh") else {
                unsafe { libc_exit(127) };
            };
            let Ok(flag) = CString::new("-c") else {
                unsafe { libc_exit(127) };
            };
            let Ok(script) = CString::new(raw.as_str()) else {
                unsafe { libc_exit(127) };
            };
            let args = [sh.as_c_str(), flag.as_c_str(), script.as_c_str()];
            let c_env = build_c_env(session, &[]);
            let Err(errno) = execvpe(&sh, &args, &c_env);
            eprintln!("koa: sh: {}", errno.desc());
            unsafe { libc_exit(127) };
        }
        Command::Simple(simple) => {
            let mut extra_env = Vec::with_capacity(simple.assignments.len());
            for assign in &simple.assignments {
                let val = expand_word(&assign.value, &session.env, session.last_status);
                extra_env.push((assign.name.clone(), val));
            }

            if let Err(msg) = apply_redirections(session, &simple.redirections) {
                eprintln!("koa: {msg}");
                unsafe { libc_exit(1) };
            }

            let argv_strings: Vec<String> = simple
                .words
                .iter()
                .map(|w| expand_word(w, &session.env, session.last_status))
                .collect();

            if argv_strings.is_empty() {
                unsafe { libc_exit(0) };
            }

            let mut c_args = Vec::with_capacity(argv_strings.len());
            for s in &argv_strings {
                match CString::new(s.as_str()) {
                    Ok(cs) => c_args.push(cs),
                    Err(e) => {
                        eprintln!("koa: invalid argument: {e}");
                        unsafe { libc_exit(127) };
                    }
                }
            }

            let prog = &c_args[0];
            let c_env = build_c_env(session, &extra_env);
            let Err(errno) = execvpe(prog, &c_args, &c_env);
            let exit_code = match errno {
                nix::errno::Errno::EACCES => 126,
                _ => 127,
            };
            eprintln!("koa: {}: {}", argv_strings[0], errno.desc());
            unsafe { libc_exit(exit_code) };
        }
    }
}

fn apply_redirections(session: &Session, redirs: &[Redirection]) -> Result<(), String> {
    for redir in redirs {
        match redir.kind {
            RedirectKind::StderrToStdout => {
                dup2(1, 2).map_err(|e| format!("dup2 2>&1 failed: {e}"))?;
            }
            kind => {
                let Some(ref target_word) = redir.target else {
                    return Err("missing redirection target".to_string());
                };
                let path = expand_word(target_word, &session.env, session.last_status);
                let (flags, target_fds): (OFlag, &[i32]) = match kind {
                    RedirectKind::Stdin => (OFlag::O_RDONLY | OFlag::O_CLOEXEC, &[0]),
                    RedirectKind::StdoutTrunc => (
                        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC | OFlag::O_CLOEXEC,
                        &[1],
                    ),
                    RedirectKind::StdoutAppend => (
                        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_APPEND | OFlag::O_CLOEXEC,
                        &[1],
                    ),
                    RedirectKind::StderrTrunc => (
                        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC | OFlag::O_CLOEXEC,
                        &[2],
                    ),
                    RedirectKind::StderrAppend => (
                        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_APPEND | OFlag::O_CLOEXEC,
                        &[2],
                    ),
                    RedirectKind::StdoutAndStderrTrunc => (
                        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC | OFlag::O_CLOEXEC,
                        &[1, 2],
                    ),
                    RedirectKind::StderrToStdout => unreachable!(),
                };
                let mode = Mode::from_bits_truncate(0o644);
                let fd = open(path.as_str(), flags, mode)
                    .map_err(|e| format!("{path}: {e}"))?;
                for &tfd in target_fds {
                    if fd.as_raw_fd() != tfd {
                        dup2(fd.as_raw_fd(), tfd).map_err(|e| format!("dup2 failed: {e}"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn wait_for_foreground_job(
    session: &mut Session,
    pgid: Pid,
    pids: Vec<Pid>,
    cmd_summary: String,
    existing_job_id: Option<usize>,
) -> Result<i32, StructuredDiagnostic> {
    let mut remaining = pids.len();
    let mut last_code = 0;

    while remaining > 0 {
        match waitpid(Pid::from_raw(-pgid.as_raw()), Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                if Some(&pid) == pids.last() {
                    last_code = code;
                }
                remaining = remaining.saturating_sub(1);
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                if Some(&pid) == pids.last() {
                    last_code = 128 + sig as i32;
                }
                remaining = remaining.saturating_sub(1);
            }
            Ok(WaitStatus::Stopped(_pid, sig)) => {
                session.restore_terminal();
                let job_id = if let Some(id) = existing_job_id {
                    if let Some(job) = session.jobs.iter_mut().find(|j| j.id == id) {
                        job.state = JobState::Stopped;
                    }
                    id
                } else {
                    session.add_job(pgid, pids, cmd_summary.clone(), JobState::Stopped)
                };
                eprintln!("\n[{job_id}]+ Stopped                 {cmd_summary}");
                return Ok(128 + sig as i32);
            }
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(nix::errno::Errno::ECHILD) => break,
            Err(e) => {
                session.restore_terminal();
                return Err(StructuredDiagnostic::new("E_WAITPID", e.to_string(), None, None));
            }
        }
    }

    session.restore_terminal();
    if let Some(id) = existing_job_id {
        session.jobs.retain(|j| j.id != id);
    }
    Ok(last_code)
}

pub fn reap_background_jobs(session: &mut Session) {
    loop {
        match waitpid(
            Pid::from_raw(-1),
            Some(WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED),
        ) {
            Ok(WaitStatus::Exited(pid, code)) => {
                mark_pid_done(session, pid, code);
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                mark_pid_done(session, pid, 128 + sig as i32);
            }
            Ok(WaitStatus::Stopped(pid, _)) => {
                for job in &mut session.jobs {
                    if job.pids.contains(&pid) {
                        job.state = JobState::Stopped;
                    }
                }
            }
            _ => break,
        }
    }
    session.jobs.retain(|j| !matches!(j.state, JobState::Done(_)));
}

fn mark_pid_done(session: &mut Session, pid: Pid, code: i32) {
    for job in &mut session.jobs {
        if let Some(pos) = job.pids.iter().position(|&p| p == pid) {
            job.pids.remove(pos);
            if job.pids.is_empty() {
                job.state = JobState::Done(code);
                eprintln!("[{}]+ Done ({code})               {}", job.id, job.command);
            }
            break;
        }
    }
}

fn try_run_kernel_builtin(
    session: &mut Session,
    cmd: &SimpleCommand,
) -> Result<Option<i32>, StructuredDiagnostic> {
    if cmd.words.is_empty() {
        for assign in &cmd.assignments {
            let val = expand_word(&assign.value, &session.env, session.last_status);
            session.env.insert(assign.name.clone(), val);
        }
        return Ok(Some(0));
    }

    let args: Vec<String> = cmd
        .words
        .iter()
        .map(|w| expand_word(w, &session.env, session.last_status))
        .collect();
    let name = args[0].as_str();

    match name {
        "cd" => {
            let raw = if args.len() > 1 {
                args[1].clone()
            } else {
                session
                    .env
                    .get("HOME")
                    .unwrap_or_else(|| "/".to_string())
            };
            let target = if std::path::Path::new(&raw).is_absolute() {
                std::path::PathBuf::from(&raw)
            } else {
                session.cwd.join(&raw)
            };
            match std::fs::canonicalize(&target) {
                Ok(canonical) if canonical.is_dir() => {
                    let s = canonical.to_string_lossy().to_string();
                    session.cwd = canonical;
                    session.env.insert("PWD".to_string(), s);
                    Ok(Some(0))
                }
                Ok(_) => {
                    eprintln!("cd: {raw}: Not a directory");
                    Ok(Some(1))
                }
                Err(e) => {
                    eprintln!("cd: {raw}: {e}");
                    Ok(Some(1))
                }
            }
        }
        "pwd" => {
            println!("{}", session.cwd.display());
            Ok(Some(0))
        }
        "export" => {
            for arg in args.iter().skip(1) {
                if let Some((k, v)) = arg.split_once('=') {
                    session.env.insert(k.to_string(), v.to_string());
                }
            }
            Ok(Some(0))
        }
        "unset" => {
            for arg in args.iter().skip(1) {
                session.env.remove(arg);
            }
            Ok(Some(0))
        }
        "exit" => {
            let code = args
                .get(1)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(session.last_status);
            session.should_exit = Some(code);
            Ok(Some(code))
        }
        "jobs" => {
            for job in &session.jobs {
                let state_str = match job.state {
                    JobState::Running => "Running",
                    JobState::Stopped => "Stopped",
                    JobState::Done(_) => "Done",
                };
                println!("[{}]+ {state_str:<23} {}", job.id, job.command);
            }
            Ok(Some(0))
        }
        "fg" => {
            let target_id = args.get(1).and_then(|s| s.trim_start_matches('%').parse::<usize>().ok());
            let job_idx = match target_id {
                Some(id) => session.jobs.iter().position(|j| j.id == id),
                None => session.jobs.len().checked_sub(1),
            };
            let Some(idx) = job_idx else {
                eprintln!("fg: no current job");
                return Ok(Some(1));
            };
            let job = session.jobs[idx].clone();
            eprintln!("{}", job.command);
            if session.interactive {
                let stdin = std::io::stdin();
                let _ = tcsetpgrp(&stdin, job.pgid);
            }
            let _ = killpg(job.pgid, Some(Signal::SIGCONT));
            let status = wait_for_foreground_job(
                session,
                job.pgid,
                job.pids,
                job.command,
                Some(job.id),
            )?;
            Ok(Some(status))
        }
        "bg" => {
            let target_id = args.get(1).and_then(|s| s.trim_start_matches('%').parse::<usize>().ok());
            let job_idx = match target_id {
                Some(id) => session.jobs.iter().position(|j| j.id == id),
                None => session.jobs.iter().rposition(|j| j.state == JobState::Stopped),
            };
            let Some(idx) = job_idx else {
                eprintln!("bg: no stopped job");
                return Ok(Some(1));
            };
            let job_id = session.jobs[idx].id;
            let pgid = session.jobs[idx].pgid;
            let cmd_str = session.jobs[idx].command.clone();
            session.jobs[idx].state = JobState::Running;
            let _ = killpg(pgid, Some(Signal::SIGCONT));
            eprintln!("[{job_id}]+ {cmd_str} &");
            Ok(Some(0))
        }
        _ => Ok(None),
    }
}

fn summarize_pipeline(session: &Session, pipeline: &Pipeline) -> String {
    pipeline
        .commands
        .iter()
        .map(|c| match c {
            Command::Simple(s) => s
                .words
                .iter()
                .map(|w| expand_word(w, &session.env, session.last_status))
                .collect::<Vec<_>>()
                .join(" "),
            Command::PosixFallback { raw, .. } => raw.clone(),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

unsafe fn libc_exit(code: i32) -> ! {
    unsafe {
        nix::libc::_exit(code);
    }
}
