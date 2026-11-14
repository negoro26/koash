use std::io::ErrorKind;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use nix::libc;
use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
use nix::unistd::Pid;
use shell_value::StructuredDiagnostic;

use crate::kill::wait_group;
use crate::session::{JobState, Session};

#[derive(Debug, Clone, Copy)]
pub enum ChildGroup {
    Shared,
    Leader,
    Join(Pid),
}

#[derive(Debug, Clone, Copy)]
pub struct ChildSetup {
    pub group: ChildGroup,
    pub tty: Option<RawFd>,
    pub die_with_parent: bool,
}

pub fn prepare_child(command: &mut Command, setup: ChildSetup) {
    match setup.group {
        ChildGroup::Shared => {}
        ChildGroup::Leader => {
            command.process_group(0);
        }
        ChildGroup::Join(pgid) => {
            command.process_group(pgid.as_raw());
        }
    }

    let ChildSetup {
        tty,
        die_with_parent,
        ..
    } = setup;
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: between fork and exec only async-signal-safe libc calls run, without allocating.
    unsafe {
        command.pre_exec(move || {
            // The shell ignores SIGTTOU, so claiming the terminal must come before the reset below.
            if let Some(fd) = tty {
                libc::tcsetpgrp(fd, libc::getpid());
            }
            if die_with_parent {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                // The parent may already be gone, in which case the signal never comes.
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
            }
            for sig in [
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTSTP,
                libc::SIGTTIN,
                libc::SIGTTOU,
            ] {
                libc::signal(sig, libc::SIG_DFL);
            }
            Ok(())
        });
    }
}

pub struct ResolvedSpawn<'a> {
    pub program: &'a str,
    pub argv0: Option<&'a str>,
    pub args: &'a [String],
    pub cwd: &'a std::path::Path,
    pub env: &'a [(String, String)],
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
    pub group: ChildGroup,
    pub tty: Option<RawFd>,
}

/// The one place koa turns a fully resolved command (cwd, complete environment, stdio) into a child process.
pub fn spawn_resolved(spec: ResolvedSpawn) -> std::io::Result<Child> {
    let mut command = Command::new(spec.program);
    if let Some(argv0) = spec.argv0 {
        command.arg0(argv0);
    }
    command
        .args(spec.args)
        .current_dir(spec.cwd)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(spec.stdin)
        .stdout(spec.stdout)
        .stderr(spec.stderr);
    prepare_child(
        &mut command,
        ChildSetup {
            group: spec.group,
            tty: spec.tty,
            die_with_parent: false,
        },
    );
    command.spawn()
}

pub struct ExternalSpec<'a> {
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
    pub group: ChildGroup,
    pub claim_terminal: bool,
}

#[derive(Debug)]
pub struct SpawnError {
    pub status: i32,
    pub diag: StructuredDiagnostic,
}

pub fn spawn_external(session: &Session, spec: ExternalSpec) -> Result<Child, SpawnError> {
    let Some((program, args)) = spec.argv.split_first() else {
        return Err(SpawnError {
            status: 127,
            diag: StructuredDiagnostic::new("E_SPAWN", "empty command", None, None),
        });
    };

    // A failed chdir in the child is indistinguishable from a missing program, so check up front.
    if matches!(std::fs::metadata(&session.cwd), Err(e) if e.kind() == ErrorKind::NotFound) {
        return Err(SpawnError {
            status: 1,
            diag: StructuredDiagnostic::new(
                "E_CWD",
                format!(
                    "current directory {} no longer exists",
                    session.cwd.display()
                ),
                None,
                None,
            ),
        });
    }

    let env: Vec<(String, String)> = session
        .env
        .materialize()
        .into_iter()
        .chain(spec.env.iter().cloned())
        .collect();
    let tty = spec
        .claim_terminal
        .then(|| session.tty.as_ref().map(AsRawFd::as_raw_fd))
        .flatten();
    let child = spawn_resolved(ResolvedSpawn {
        program,
        argv0: None,
        args,
        cwd: &session.cwd,
        env: &env,
        stdin: spec.stdin,
        stdout: spec.stdout,
        stderr: spec.stderr,
        group: spec.group,
        tty,
    })
    .map_err(|e| {
        let (status, code, what) = match e.kind() {
            ErrorKind::NotFound if program.contains('/') => {
                (127, "E_NOT_FOUND", "No such file or directory".to_string())
            }
            ErrorKind::NotFound => (127, "E_NOT_FOUND", "command not found".to_string()),
            ErrorKind::PermissionDenied => {
                (126, "E_NOT_EXECUTABLE", "Permission denied".to_string())
            }
            _ => (127, "E_SPAWN", e.to_string()),
        };
        SpawnError {
            status,
            diag: StructuredDiagnostic::new(code, format!("{program}: {what}"), None, None),
        }
    })?;
    if spec.claim_terminal {
        // The child does the same before exec; whichever runs first wins, so neither can lose the race.
        session.give_terminal(Pid::from_raw(child.id() as i32));
    }
    Ok(child)
}

#[derive(Debug)]
pub struct Stopped {
    pub signal: i32,
    pub alive: Vec<Pid>,
}

#[derive(Debug)]
pub struct ForegroundOutcome {
    pub watched_status: i32,
    pub stopped: Option<Stopped>,
}

/// Waits on `pgid` only, never a bare `waitpid(-1)`, so unrelated children are not reaped.
pub fn wait_foreground(pgid: Pid, pids: &[Pid], watch: Option<Pid>) -> ForegroundOutcome {
    let mut alive = pids.to_vec();
    let mut watched_status = 0;
    while !alive.is_empty() {
        match wait_group(pgid, true) {
            Ok(Some(WaitStatus::Exited(pid, code))) => {
                alive.retain(|&p| p != pid);
                if Some(pid) == watch {
                    watched_status = code;
                }
            }
            Ok(Some(WaitStatus::Signaled(pid, sig, _))) => {
                alive.retain(|&p| p != pid);
                if Some(pid) == watch {
                    watched_status = 128 + sig as i32;
                }
            }
            Ok(Some(WaitStatus::Stopped(_, sig))) => {
                return ForegroundOutcome {
                    watched_status,
                    stopped: Some(Stopped {
                        signal: sig as i32,
                        alive,
                    }),
                };
            }
            Ok(Some(_)) | Ok(None) | Err(nix::errno::Errno::EINTR) => {}
            Err(_) => break,
        }
    }
    ForegroundOutcome {
        watched_status,
        stopped: None,
    }
}

pub fn group_stopped(pgid: Pid) -> bool {
    matches!(
        waitid(
            Id::PGid(pgid),
            WaitPidFlag::WSTOPPED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
        ),
        Ok(WaitStatus::Stopped(..))
    )
}

pub fn resume_group(pgid: Pid) {
    let _ = killpg(pgid, Signal::SIGCONT);
}

pub fn reap_background_jobs(session: &mut Session) -> Vec<String> {
    let mut notices = Vec::new();
    for job in &mut session.jobs {
        if matches!(job.state, JobState::Done(_)) {
            continue;
        }
        loop {
            let finished = match wait_group(job.pgid, false) {
                Ok(Some(WaitStatus::Exited(pid, code))) => Some((pid, code)),
                Ok(Some(WaitStatus::Signaled(pid, sig, _))) => Some((pid, 128 + sig as i32)),
                Ok(Some(WaitStatus::Stopped(..))) => {
                    job.state = JobState::Stopped;
                    None
                }
                Ok(Some(_)) => None,
                Ok(None) => break,
                Err(nix::errno::Errno::EINTR) => None,
                Err(_) => {
                    job.pids.clear();
                    Some((Pid::from_raw(0), 0))
                }
            };
            if let Some((pid, code)) = finished {
                job.pids.retain(|&p| p != pid);
                if job.pids.is_empty() {
                    job.state = JobState::Done(code);
                    notices.push(format!(
                        "[{}]+ Done ({code})               {}",
                        job.id, job.command
                    ));
                    break;
                }
            }
        }
    }
    session
        .jobs
        .retain(|j| !matches!(j.state, JobState::Done(_)));
    notices
}
