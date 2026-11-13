use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use nix::libc;
use nix::sys::wait::WaitStatus;
use nix::unistd::Pid;
use shell_value::StructuredDiagnostic;

use crate::kill::KillSwitch;
use crate::session::{JobState, Session};

pub struct ExternalSpec<'a> {
    pub argv: &'a [String],
    pub env: &'a [(String, String)],
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
    /// Join this process group; `None` makes the child the leader of a new one.
    pub pgid: Option<Pid>,
    /// Hand the controlling terminal to the new group before exec.
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

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&session.cwd)
        .env_clear()
        .envs(session.env.materialize())
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(spec.stdin)
        .stdout(spec.stdout)
        .stderr(spec.stderr)
        .process_group(spec.pgid.map_or(0, Pid::as_raw));

    let claim_terminal = spec.claim_terminal;
    // SAFETY: the closure runs between fork and exec and only calls async-signal-safe libc
    // functions (tcsetpgrp, getpid, signal) without allocating.
    unsafe {
        command.pre_exec(move || {
            // The shell ignores SIGTTOU, so claiming the terminal must come before the reset below.
            if claim_terminal {
                libc::tcsetpgrp(0, libc::getpid());
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

    command.spawn().map_err(|e| {
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
    })
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

/// Waits on `pgid` only (never `waitpid(-1)`) until every pid in `pids` has exited or the group stops.
pub fn wait_foreground(
    kill: &KillSwitch,
    pgid: Pid,
    pids: &[Pid],
    watch: Option<Pid>,
) -> ForegroundOutcome {
    let mut alive = pids.to_vec();
    let mut watched_status = 0;
    while !alive.is_empty() {
        match kill.wait_group(pgid, true) {
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
                kill.untrack(pgid);
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
    kill.untrack(pgid);
    ForegroundOutcome {
        watched_status,
        stopped: None,
    }
}

/// Collects finished and stopped background jobs, waiting on each job's own pgid only.
/// Returns one notice line per job that finished.
pub fn reap_background_jobs(session: &mut Session) -> Vec<String> {
    let kill = session.kill.clone();
    let mut notices = Vec::new();
    for job in &mut session.jobs {
        if matches!(job.state, JobState::Done(_)) {
            continue;
        }
        loop {
            let finished = match kill.wait_group(job.pgid, false) {
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
