use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nix::sys::signal::{SigHandler, Signal, signal};
use nix::sys::termios::{Termios, tcgetattr};
use nix::unistd::{Pid, getpgrp, isatty, setpgid, tcgetpgrp, tcsetpgrp};
use serde::{Deserialize, Serialize};
use shell_value::{StructuredDiagnostic, TaintLevel};

use crate::cow::CowWorkspace;
use crate::env::SessionEnv;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Done(i32),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: usize,
    pub pgid: Pid,
    pub pids: Vec<Pid>,
    pub command: String,
    pub state: JobState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub cwd: PathBuf,
    pub env: std::collections::BTreeMap<String, String>,
    pub last_status: i32,
    pub mutated_files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TxRecord {
    pub id: String,
    pub command: String,
    pub preimages: Vec<(PathBuf, Option<Vec<u8>>)>,
}

pub struct Session {
    pub cwd: PathBuf,
    pub env: SessionEnv,
    pub last_status: i32,
    pub interactive: bool,
    pub shell_pgid: Pid,
    pub saved_termios: Mutex<Option<Termios>>,
    pub jobs: Vec<Job>,
    pub next_job_id: usize,
    pub should_exit: Option<i32>,
    pub mutated_files: Vec<String>,
    pub tx_log: Vec<TxRecord>,
    pub next_tx_id: usize,
    pub taint: TaintLevel,
    pub cow: Option<CowWorkspace>,
    pub last_active_at: Instant,
    pub idle_ttl: Duration,
    pub ephemeral: bool,
    pub parent_id: Option<String>,
}

impl Session {
    pub fn new(interactive: bool) -> Result<Self, StructuredDiagnostic> {
        let cwd = std::env::current_dir()
            .map_err(|e| StructuredDiagnostic::from_io("E_GETCWD", &e, None))?;
        let env = SessionEnv::new();

        let stdin = std::io::stdin();
        let is_tty = isatty(0).unwrap_or(false);
        let interactive_tty = interactive && is_tty;

        let mut shell_pgid = getpgrp();
        let mut saved_termios = None;

        if interactive_tty {
            loop {
                let fg_pgid = tcgetpgrp(&stdin).unwrap_or(shell_pgid);
                if fg_pgid == shell_pgid {
                    break;
                }
                let _ = nix::sys::signal::killpg(shell_pgid, Some(Signal::SIGTTIN));
            }

            unsafe {
                let _ = signal(Signal::SIGINT, SigHandler::SigIgn);
                let _ = signal(Signal::SIGQUIT, SigHandler::SigIgn);
                let _ = signal(Signal::SIGTSTP, SigHandler::SigIgn);
                let _ = signal(Signal::SIGTTIN, SigHandler::SigIgn);
                let _ = signal(Signal::SIGTTOU, SigHandler::SigIgn);
            }

            let my_pid = nix::unistd::getpid();
            if shell_pgid != my_pid {
                let _ = setpgid(my_pid, my_pid);
                shell_pgid = my_pid;
            }
            let _ = tcsetpgrp(&stdin, shell_pgid);
            saved_termios = tcgetattr(&stdin).ok();
        }

        Ok(Self {
            cwd,
            env,
            last_status: 0,
            interactive: interactive_tty,
            shell_pgid,
            saved_termios: Mutex::new(saved_termios),
            jobs: Vec::new(),
            next_job_id: 1,
            should_exit: None,
            mutated_files: Vec::new(),
            tx_log: Vec::new(),
            next_tx_id: 1,
            taint: TaintLevel::TrustedLocal,
            cow: None,
            last_active_at: Instant::now(),
            idle_ttl: Duration::from_secs(900),
            ephemeral: false,
            parent_id: None,
        })
    }

    pub fn fork_detached(&self) -> Self {
        Self {
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            last_status: self.last_status,
            interactive: false,
            shell_pgid: self.shell_pgid,
            saved_termios: Mutex::new(None),
            jobs: Vec::new(),
            next_job_id: 1,
            should_exit: None,
            mutated_files: Vec::new(),
            tx_log: Vec::new(),
            next_tx_id: self.next_tx_id,
            taint: self.taint,
            cow: None,
            last_active_at: Instant::now(),
            idle_ttl: Duration::from_secs(300),
            ephemeral: true,
            parent_id: None,
        }
    }

    pub fn fork_isolated(
        &self,
        parent_id: Option<String>,
        new_id: &str,
    ) -> Result<Self, StructuredDiagnostic> {
        let mut child = self.fork_detached();
        child.parent_id = parent_id;
        child.cow = Some(CowWorkspace::new(&self.cwd, new_id)?);
        Ok(child)
    }

    pub fn touch(&mut self) {
        self.last_active_at = Instant::now();
    }

    pub fn is_expired(&self) -> bool {
        self.jobs.is_empty() && self.last_active_at.elapsed() > self.idle_ttl
    }

    pub fn snapshot(&self) -> Result<String, StructuredDiagnostic> {
        let snap = SessionSnapshot {
            cwd: self.cwd.clone(),
            env: self.env.materialize(),
            last_status: self.last_status,
            mutated_files: self.mutated_files.clone(),
        };
        serde_json::to_string(&snap)
            .map_err(|e| StructuredDiagnostic::new("E_SNAPSHOT", e.to_string(), None, None))
    }

    pub fn restore(json: &str) -> Result<Self, StructuredDiagnostic> {
        let snap: SessionSnapshot = serde_json::from_str(json)
            .map_err(|e| StructuredDiagnostic::new("E_RESTORE", e.to_string(), None, None))?;
        Ok(Self {
            cwd: snap.cwd,
            env: SessionEnv::from_map(snap.env),
            last_status: snap.last_status,
            interactive: false,
            shell_pgid: getpgrp(),
            saved_termios: Mutex::new(None),
            jobs: Vec::new(),
            next_job_id: 1,
            should_exit: None,
            mutated_files: snap.mutated_files,
            tx_log: Vec::new(),
            next_tx_id: 1,
            taint: TaintLevel::TrustedLocal,
            cow: None,
            last_active_at: Instant::now(),
            idle_ttl: Duration::from_secs(900),
            ephemeral: false,
            parent_id: None,
        })
    }

    pub fn begin_tx(&mut self, command: impl Into<String>) -> String {
        if self.tx_log.len() >= 50 {
            self.tx_log.remove(0);
        }
        let id = format!("tx_{:03}", self.next_tx_id);
        self.next_tx_id += 1;
        self.tx_log.push(TxRecord {
            id: id.clone(),
            command: command.into(),
            preimages: Vec::new(),
        });
        id
    }

    pub fn record_preimage(&mut self, path: &Path) {
        let Some(tx) = self.tx_log.last_mut() else {
            return;
        };
        if tx.preimages.iter().any(|(p, _)| p == path) {
            return;
        }
        let data = if path.is_file() {
            std::fs::read(path).ok()
        } else {
            None
        };
        tx.preimages.push((path.to_path_buf(), data));
    }

    pub fn finish_tx(&mut self) {
        if self.tx_log.last().is_some_and(|t| t.preimages.is_empty()) {
            self.tx_log.pop();
        }
    }

    pub fn rollback_tx(&mut self, tx_id: &str) -> Result<usize, StructuredDiagnostic> {
        let pos = self
            .tx_log
            .iter()
            .position(|t| t.id == tx_id)
            .ok_or_else(|| {
                StructuredDiagnostic::new(
                    "E_TX_NOT_FOUND",
                    format!("transaction `{tx_id}` not found"),
                    None,
                    None,
                )
            })?;
        let drained: Vec<TxRecord> = self.tx_log.drain(pos..).collect();
        let mut restored = 0usize;
        for tx in drained.into_iter().rev() {
            for (path, pre) in tx.preimages.into_iter().rev() {
                match pre {
                    Some(bytes) => {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        std::fs::write(&path, bytes)
                            .map_err(|e| StructuredDiagnostic::from_io("E_ROLLBACK", &e, None))?;
                    }
                    None => {
                        if path.is_file() {
                            let _ = std::fs::remove_file(&path);
                        } else if path.is_dir() {
                            let _ = std::fs::remove_dir_all(&path);
                        }
                    }
                }
                restored += 1;
            }
        }
        Ok(restored)
    }

    pub fn undo_last(&mut self) -> Result<Option<(String, usize)>, StructuredDiagnostic> {
        let Some(last) = self.tx_log.last().map(|t| t.id.clone()) else {
            return Ok(None);
        };
        let count = self.rollback_tx(&last)?;
        Ok(Some((last, count)))
    }

    pub fn record_mutation(&mut self, path: impl Into<String>) {
        let p = path.into();
        if !self.mutated_files.contains(&p) {
            self.mutated_files.push(p);
        }
    }

    pub fn add_job(&mut self, pgid: Pid, pids: Vec<Pid>, command: String, state: JobState) -> usize {
        let id = self.next_job_id;
        self.next_job_id += 1;
        self.jobs.push(Job {
            id,
            pgid,
            pids,
            command,
            state,
        });
        id
    }

    pub fn restore_terminal(&self) {
        if !self.interactive {
            return;
        }
        let stdin = std::io::stdin();
        let _ = tcsetpgrp(&stdin, self.shell_pgid);
        if let Ok(guard) = self.saved_termios.lock()
            && let Some(ref termios) = *guard
        {
            let _ = nix::sys::termios::tcsetattr(
                &stdin,
                nix::sys::termios::SetArg::TCSADRAIN,
                termios,
            );
        }
    }
}
