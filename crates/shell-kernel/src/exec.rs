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

/// Which process group a child lands in.
#[derive(Debug, Clone, Copy)]
pub enum ChildGroup {
    /// Stay in the shell's group, like `sh -c` without job control.
    Shared,
    /// Become the leader of a new group.
    Leader,
    Join(Pid),
}

/// What a child needs between fork and exec.
#[derive(Debug, Clone, Copy)]
pub struct ChildSetup {
    pub group: ChildGroup,
    /// Terminal fd to hand to the child's own group before exec.
    pub tty: Option<RawFd>,
    /// Kill the child when the process that spawned it dies.
    pub die_with_parent: bool,
}

/// Applies `setup` to `command`. The job-control signals the shell ignores go back to their
/// default so the child reacts to the terminal like any other program.
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
    // SAFETY: the closure runs between fork and exec and only calls async-signal-safe libc
    // functions (tcsetpgrp, getpid, prctl, getppid, signal) without allocating.
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

pub struct ExternalSpec<'a> {
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
    pub group: ChildGroup,
    /// Hand the controlling terminal to the new group.
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
                format!("current directory {} no longer exists", session.cwd.display()),
                None,
                None,
            ),
        });
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&session.cwd)
        .env_clear()
        .envs(session.env.materialize())
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(spec.stdin)
        .stdout(spec.stdout)
        .stderr(spec.stderr);
    let tty = spec
        .claim_terminal
        .then(|| session.tty.as_ref().map(AsRawFd::as_raw_fd))
        .flatten();
    prepare_child(
        &mut command,
        ChildSetup {
            group: spec.group,
            tty,
            die_with_parent: false,
        },
    );

    let child = command.spawn().map_err(|e| {
        let (status, code, what) = match e.kind() {
            ErrorKind::NotFound if program.contains('/') => {
                (127, "E_NOT_FOUND", "No such file or directory".to_string())
            }
            ErrorKind::NotFound => (127, "E_NOT_FOUND", "command not found".to_string()),
            ErrorKind::PermissionDenied => (126, "E_NOT_EXECUTABLE", "Permission denied".to_string()),
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
    /// Exit status of the watched process; 0 when it never reported.
    pub watched_status: i32,
    pub stopped: Option<Stopped>,
}

/// Waits on `pgid` only (never a bare `waitpid(-1)`) until every pid in `pids` has exited or the group stops.
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

/// True while a member of `pgid` is stopped. The stop stays pending for a later wait.
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

/// Collects finished and stopped background jobs, waiting on each job's own pgid only.
/// Returns one notice line per job that finished.
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
                    notices.push(format!("[{}]+ Done ({code})               {}", job.id, job.command));
                    break;
                }
            }
        }
    }
    session.jobs.retain(|j| !matches!(j.state, JobState::Done(_)));
    notices
}
