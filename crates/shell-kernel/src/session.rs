use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use nix::sys::signal::{SigHandler, Signal, signal};
use nix::sys::termios::{Termios, tcgetattr};
use nix::unistd::{Pid, getpgrp, isatty, setpgid, tcgetpgrp, tcsetpgrp};
use serde::{Deserialize, Serialize};
use shell_value::{StructuredDiagnostic, TaintLevel};

use crate::cow::CowWorkspace;
use crate::env::SessionEnv;
use crate::fs_util::reflink_or_copy;
use crate::kill::KillSwitch;
use crate::undo_store::{Snapshot, SnapshotError, UndoStore};

fn restore_mode(path: &Path, mode: u32) -> Result<(), StructuredDiagnostic> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| StructuredDiagnostic::from_io("E_ROLLBACK", &e, None))
}

fn next_session_uid() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn undo_store_for(root: PathBuf, uid: u64) -> UndoStore {
    UndoStore::new(root, format!("{}-{uid}", std::process::id()))
}

const CAPTURE_LIMIT: &str = "capture limit exceeded";

fn stat_for_capture(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot stat {}: {e}", path.display())),
    }
}

/// Records what `path` holds now. A regular file is saved as a snapshot that may write at most
/// `copy_budget` bytes.
fn save_state(
    store: &mut UndoStore,
    path: &Path,
    meta: Option<&std::fs::Metadata>,
    copy_budget: u64,
) -> Result<Preimage, String> {
    let Some(meta) = meta else {
        return Ok(Preimage::Absent);
    };
    let mode = meta.mode() & 0o7777;
    if meta.is_dir() {
        return Ok(Preimage::Dir { mode });
    }
    if !meta.is_file() {
        return Err(format!(
            "{} is not a regular file or directory",
            path.display()
        ));
    }
    match store.snapshot(path, meta.len(), copy_budget) {
        Ok(snapshot) => Ok(Preimage::File { snapshot, mode }),
        Err(SnapshotError::OverBudget) => Err(CAPTURE_LIMIT.to_string()),
        Err(SnapshotError::Io(e)) => Err(format!("cannot snapshot {}: {e}", path.display())),
    }
}

fn restore_state(path: &Path, state: &Preimage) -> Result<(), StructuredDiagnostic> {
    match state {
        Preimage::File { snapshot, mode } => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            reflink_or_copy(snapshot.path(), path)
                .map_err(|e| StructuredDiagnostic::from_io("E_ROLLBACK", &e, None))?;
            restore_mode(path, *mode)?;
        }
        Preimage::Dir { mode } => {
            std::fs::create_dir_all(path)
                .map_err(|e| StructuredDiagnostic::from_io("E_ROLLBACK", &e, None))?;
            restore_mode(path, *mode)?;
        }
        Preimage::Absent => match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => {
                let _ = std::fs::remove_dir_all(path);
            }
            Ok(_) => {
                let _ = std::fs::remove_file(path);
            }
            Err(_) => {}
        },
    }
    Ok(())
}

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

/// What a path held at one moment. A file's bytes live in a snapshot on disk.
#[derive(Debug)]
pub enum Preimage {
    Absent,
    File { snapshot: Snapshot, mode: u32 },
    Dir { mode: u32 },
}

impl Preimage {
    fn copied_bytes(&self) -> u64 {
        match self {
            Self::File { snapshot, .. } => snapshot.copied_bytes(),
            Self::Absent | Self::Dir { .. } => 0,
        }
    }
}

/// Bytes one record may write when the filesystem cannot share blocks. Shared blocks are free.
const TX_CAPTURE_MAX_COPY_BYTES: u64 = 64 * 1024 * 1024;
const TX_CAPTURE_MAX_ENTRIES: usize = 10_000;
const LEDGER_MAX_RECORDS: usize = 50;

#[derive(Debug)]
pub struct TxRecord {
    pub id: String,
    pub command: String,
    pub preimages: Vec<(PathBuf, Preimage)>,
    pub opaque_after: Option<String>,
    index: HashMap<PathBuf, usize>,
    copied_bytes: u64,
}

/// Transactions that `undo` or `rollback` took off the ledger, with the state they replaced.
/// `redo` puts both back.
#[derive(Debug)]
struct RedoRecord {
    txs: Vec<TxRecord>,
    replaced: Vec<(PathBuf, Preimage)>,
}

pub struct Session {
    pub cwd: PathBuf,
    pub env: SessionEnv,
    pub last_status: i32,
    pub interactive: bool,
    pub shell_pgid: Pid,
    pub saved_termios: Mutex<Option<Termios>>,
    pub tty: Option<OwnedFd>,
    pub jobs: Vec<Job>,
    pub next_job_id: usize,
    pub should_exit: Option<i32>,
    pub mutated_files: Vec<String>,
    pub tx_log: Vec<TxRecord>,
    redo_log: Vec<RedoRecord>,
    undo_store: UndoStore,
    pub next_tx_id: usize,
    pub taint: TaintLevel,
    pub cow: Option<CowWorkspace>,
    pub last_active_at: Instant,
    pub idle_ttl: Duration,
    pub ephemeral: bool,
    pub parent_id: Option<String>,
    pub kill: KillSwitch,
    pub uid: u64,
    reports: Vec<StructuredDiagnostic>,
}

impl Session {
    pub fn new(interactive: bool) -> Result<Self, StructuredDiagnostic> {
        let cwd = std::env::current_dir()
            .map_err(|e| StructuredDiagnostic::from_io("E_GETCWD", &e, None))?;
        let env = SessionEnv::new();

        let stdin = std::io::stdin();
        let is_tty = isatty(0).unwrap_or(false);
        let interactive_tty = interactive && is_tty;
        let tty = if interactive_tty {
            let fd = stdin
                .as_fd()
                .try_clone_to_owned()
                .map_err(|e| StructuredDiagnostic::from_io("E_TTY", &e, None))?;
            Some(fd)
        } else {
            None
        };

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

        let uid = next_session_uid();
        let undo_store = undo_store_for(cwd.clone(), uid);
        Ok(Self {
            cwd,
            env,
            last_status: 0,
            interactive: interactive_tty,
            shell_pgid,
            saved_termios: Mutex::new(saved_termios),
            tty,
            jobs: Vec::new(),
            next_job_id: 1,
            should_exit: None,
            mutated_files: Vec::new(),
            tx_log: Vec::new(),
            redo_log: Vec::new(),
            undo_store,
            next_tx_id: 1,
            taint: TaintLevel::TrustedLocal,
            cow: None,
            last_active_at: Instant::now(),
            idle_ttl: Duration::from_secs(900),
            ephemeral: false,
            parent_id: None,
            kill: KillSwitch::default(),
            uid,
            reports: Vec::new(),
        })
    }

    /// Records a failure that does not stop the running builtin. A command that handles
    /// several operands reports each failed one here and carries on; the executor prints
    /// the reports after the command returns and turns them into exit status 1.
    pub fn report(&mut self, diag: StructuredDiagnostic) {
        self.reports.push(diag);
    }

    pub fn take_reports(&mut self) -> Vec<StructuredDiagnostic> {
        std::mem::take(&mut self.reports)
    }

    pub fn fork_detached(&self) -> Self {
        let uid = next_session_uid();
        Self {
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            last_status: self.last_status,
            interactive: false,
            shell_pgid: self.shell_pgid,
            saved_termios: Mutex::new(None),
            tty: None,
            jobs: Vec::new(),
            next_job_id: 1,
            should_exit: None,
            mutated_files: Vec::new(),
            tx_log: Vec::new(),
            redo_log: Vec::new(),
            undo_store: undo_store_for(self.cwd.clone(), uid),
            next_tx_id: self.next_tx_id,
            taint: self.taint,
            cow: None,
            last_active_at: Instant::now(),
            idle_ttl: Duration::from_secs(300),
            ephemeral: true,
            parent_id: None,
            kill: KillSwitch::default(),
            uid,
            reports: Vec::new(),
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
        let uid = next_session_uid();
        let undo_store = undo_store_for(snap.cwd.clone(), uid);
        Ok(Self {
            cwd: snap.cwd,
            env: SessionEnv::from_map(snap.env),
            last_status: snap.last_status,
            interactive: false,
            shell_pgid: getpgrp(),
            saved_termios: Mutex::new(None),
            tty: None,
            jobs: Vec::new(),
            next_job_id: 1,
            should_exit: None,
            mutated_files: snap.mutated_files,
            tx_log: Vec::new(),
            redo_log: Vec::new(),
            undo_store,
            next_tx_id: 1,
            taint: TaintLevel::TrustedLocal,
            cow: None,
            last_active_at: Instant::now(),
            idle_ttl: Duration::from_secs(900),
            ephemeral: false,
            parent_id: None,
            kill: KillSwitch::default(),
            uid,
            reports: Vec::new(),
        })
    }

    pub fn begin_tx(&mut self, command: impl Into<String>) -> String {
        self.finish_tx();
        if self.tx_log.len() >= LEDGER_MAX_RECORDS {
            self.tx_log.remove(0);
        }
        let id = format!("tx_{:03}", self.next_tx_id);
        self.next_tx_id += 1;
        self.tx_log.push(TxRecord {
            id: id.clone(),
            command: command.into(),
            preimages: Vec::new(),
            opaque_after: None,
            index: HashMap::new(),
            copied_bytes: 0,
        });
        id
    }

    pub fn record_preimage(&mut self, path: &Path) -> bool {
        match self.capture_one(path) {
            Ok(_) => true,
            Err(reason) => {
                self.mark_capture_failed(&reason);
                false
            }
        }
    }

    pub fn record_tree(&mut self, path: &Path) -> bool {
        let mut pending = vec![path.to_path_buf()];
        while let Some(next) = pending.pop() {
            match self.capture_one(&next) {
                Ok(false) => {}
                Ok(true) => match std::fs::read_dir(&next) {
                    Ok(entries) => {
                        for entry in entries {
                            match entry {
                                Ok(entry) => pending.push(entry.path()),
                                Err(e) => {
                                    self.mark_capture_failed(&format!(
                                        "cannot read {}: {e}",
                                        next.display()
                                    ));
                                    return false;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        self.mark_capture_failed(&format!("cannot read {}: {e}", next.display()));
                        return false;
                    }
                },
                Err(reason) => {
                    self.mark_capture_failed(&reason);
                    return false;
                }
            }
        }
        true
    }

    fn capture_one(&mut self, path: &Path) -> Result<bool, String> {
        let Some(tx) = self.tx_log.last_mut() else {
            return Ok(false);
        };
        let meta = stat_for_capture(path)?;
        if let Some(&idx) = tx.index.get(path) {
            let was_dir = matches!(tx.preimages[idx].1, Preimage::Dir { .. });
            return Ok(was_dir && meta.is_some_and(|m| m.is_dir()));
        }
        if tx.preimages.len() >= TX_CAPTURE_MAX_ENTRIES {
            return Err(CAPTURE_LIMIT.to_string());
        }
        let pre = save_state(
            &mut self.undo_store,
            path,
            meta.as_ref(),
            TX_CAPTURE_MAX_COPY_BYTES.saturating_sub(tx.copied_bytes),
        )?;
        let descend = matches!(pre, Preimage::Dir { .. });
        tx.copied_bytes += pre.copied_bytes();
        tx.index.insert(path.to_path_buf(), tx.preimages.len());
        tx.preimages.push((path.to_path_buf(), pre));
        self.redo_log.clear();
        Ok(descend)
    }

    /// Character devices and FIFOs hold no state to restore, so writing to them never blocks `undo`.
    pub fn record_output_target(&mut self, path: &Path) -> bool {
        let stateless = std::fs::metadata(path).is_ok_and(|m| {
            let kind = m.file_type();
            kind.is_char_device() || kind.is_fifo()
        });
        stateless || self.record_preimage(path)
    }

    fn mark_capture_failed(&mut self, reason: &str) {
        let command = self
            .tx_log
            .last()
            .map(|t| t.command.clone())
            .unwrap_or_default();
        self.mark_opaque(format!("{command}: {reason}"));
    }

    /// Something changed the filesystem in a way `undo` cannot reverse, so anything `redo` would
    /// write back is stale.
    pub fn mark_opaque(&mut self, what: String) {
        self.redo_log.clear();
        if let Some(tx) = self.tx_log.last_mut()
            && tx.opaque_after.is_none()
        {
            tx.opaque_after = Some(what);
        }
    }

    pub fn finish_tx(&mut self) {
        if self.tx_log.last().is_some_and(|t| t.preimages.is_empty())
            && let Some(empty) = self.tx_log.pop()
            && let Some(what) = empty.opaque_after
        {
            self.mark_opaque(what);
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
        // Rollback does not undo what an external did after these records, so the marker keeps guarding the new top.
        if let Some(what) = drained.iter().rev().find_map(|t| t.opaque_after.clone()) {
            self.mark_opaque(what);
        }
        let replaced = self.save_replaced(&drained);
        let mut restored = 0usize;
        for tx in drained.iter().rev() {
            for (path, pre) in tx.preimages.iter().rev() {
                restore_state(path, pre)?;
                restored += 1;
            }
        }
        if let Some(replaced) = replaced {
            if self.redo_log.len() >= LEDGER_MAX_RECORDS {
                self.redo_log.remove(0);
            }
            self.redo_log.push(RedoRecord {
                txs: drained,
                replaced,
            });
        }
        Ok(restored)
    }

    /// Saves what rolling `txs` back is about to overwrite, so `redo` can put it back. `None`
    /// when it does not fit the capture limits: the rollback still goes ahead, without a redo.
    fn save_replaced(&mut self, txs: &[TxRecord]) -> Option<Vec<(PathBuf, Preimage)>> {
        let mut saved: Vec<(PathBuf, Preimage)> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut copied = 0u64;
        for (root, pre) in txs.iter().flat_map(|t| &t.preimages) {
            // Restoring an absent path deletes whatever is there, including a whole tree.
            let descend = matches!(pre, Preimage::Absent);
            let mut pending = vec![root.clone()];
            while let Some(path) = pending.pop() {
                if !seen.insert(path.clone()) {
                    continue;
                }
                if saved.len() >= TX_CAPTURE_MAX_ENTRIES {
                    return None;
                }
                let meta = stat_for_capture(&path).ok()?;
                let state = save_state(
                    &mut self.undo_store,
                    &path,
                    meta.as_ref(),
                    TX_CAPTURE_MAX_COPY_BYTES.saturating_sub(copied),
                )
                .ok()?;
                copied += state.copied_bytes();
                if descend && matches!(state, Preimage::Dir { .. }) {
                    for entry in std::fs::read_dir(&path).ok()? {
                        pending.push(entry.ok()?.path());
                    }
                }
                saved.push((path, state));
            }
        }
        Some(saved)
    }

    /// Re-applies the most recently undone transactions: puts back the state the undo replaced and
    /// returns the transactions to the ledger. Returns the oldest of them and the paths restored.
    pub fn redo_last(&mut self) -> Result<Option<(String, usize)>, StructuredDiagnostic> {
        self.finish_tx();
        let Some(record) = self.redo_log.pop() else {
            return Ok(None);
        };
        let mut restored = 0usize;
        for (path, state) in record.replaced.iter().rev() {
            restore_state(path, state)?;
            restored += 1;
        }
        let first = record.txs.first().map(|t| t.id.clone()).unwrap_or_default();
        self.tx_log.extend(record.txs);
        if self.tx_log.len() > LEDGER_MAX_RECORDS {
            self.tx_log.drain(..self.tx_log.len() - LEDGER_MAX_RECORDS);
        }
        Ok(Some((first, restored)))
    }

    /// How many `redo` commands would find something to re-apply.
    pub fn redo_depth(&self) -> usize {
        self.redo_log.len()
    }

    pub fn undo_last(&mut self) -> Result<Option<(String, usize)>, StructuredDiagnostic> {
        self.finish_tx();
        let Some(last) = self.tx_log.last() else {
            return Ok(None);
        };
        if let Some(cmd) = &last.opaque_after {
            return Err(StructuredDiagnostic::new(
                "E_TX_OPAQUE",
                format!(
                    "\"{cmd}\" ran after {id} and is not undoable; run \"rollback {id}\" to revert {id} anyway",
                    id = last.id
                ),
                None,
                None,
            ));
        }
        let last = last.id.clone();
        let count = self.rollback_tx(&last)?;
        Ok(Some((last, count)))
    }

    pub fn record_mutation(&mut self, path: impl Into<String>) {
        let p = path.into();
        if !self.mutated_files.contains(&p) {
            self.mutated_files.push(p);
        }
    }

    pub fn add_job(
        &mut self,
        pgid: Pid,
        pids: Vec<Pid>,
        command: String,
        state: JobState,
    ) -> usize {
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

    pub fn give_terminal(&self, pgid: Pid) {
        if let Some(tty) = &self.tty {
            let _ = tcsetpgrp(tty, pgid);
        }
    }

    pub fn restore_terminal(&self) {
        let Some(tty) = &self.tty else {
            return;
        };
        let _ = tcsetpgrp(tty, self.shell_pgid);
        if let Ok(guard) = self.saved_termios.lock()
            && let Some(termios) = &*guard
        {
            let _ =
                nix::sys::termios::tcsetattr(tty, nix::sys::termios::SetArg::TCSADRAIN, termios);
        }
    }
}
