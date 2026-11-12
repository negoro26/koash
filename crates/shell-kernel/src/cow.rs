use std::borrow::Cow;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rustc_hash::FxHashMap;
use shell_value::StructuredDiagnostic;

const FICLONE: nix::libc::c_ulong = 0x4004_9409;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationState {
    Modified,
    Whiteout,
}

pub struct CowWorkspace {
    pub session_id: String,
    pub root: PathBuf,
    pub session_dir: PathBuf,
    pub work_dir: PathBuf,
    pub mutations: FxHashMap<PathBuf, MutationState>,
    _lease_file: File,
}

impl CowWorkspace {
    pub fn new(root: &Path, session_id: &str) -> Result<Self, StructuredDiagnostic> {
        let session_dir = root.join(".koa").join("sessions").join(session_id);
        let work_dir = session_dir.join("work");
        std::fs::create_dir_all(&work_dir)
            .map_err(|e| StructuredDiagnostic::from_io("E_COW_DIR", &e, None))?;

        let lock_path = session_dir.join("lease.lock");
        let lease_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| StructuredDiagnostic::from_io("E_COW_LOCK", &e, None))?;

        let rc = unsafe {
            nix::libc::flock(
                lease_file.as_raw_fd(),
                nix::libc::LOCK_EX | nix::libc::LOCK_NB,
            )
        };
        if rc != 0 {
            return Err(StructuredDiagnostic::new(
                "E_COW_LEASE",
                format!("failed to acquire lease lock for `{session_id}`"),
                None,
                None,
            ));
        }

        Ok(Self {
            session_id: session_id.to_string(),
            root: root.to_path_buf(),
            session_dir,
            work_dir,
            mutations: FxHashMap::default(),
            _lease_file: lease_file,
        })
    }

    pub fn resolve_read<'a>(&'a self, rel: &'a Path) -> Option<Cow<'a, Path>> {
        match self.mutations.get(rel) {
            Some(MutationState::Whiteout) => None,
            Some(MutationState::Modified) => Some(Cow::Owned(self.work_dir.join(rel))),
            None => Some(Cow::Borrowed(rel)),
        }
    }

    pub fn prepare_write(&mut self, rel: &Path) -> Result<PathBuf, StructuredDiagnostic> {
        let shadow = self.work_dir.join(rel);
        if let Some(parent) = shadow.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| StructuredDiagnostic::from_io("E_COW_MKDIR", &e, None))?;
        }
        if !matches!(self.mutations.get(rel), Some(MutationState::Modified)) {
            let real = self.root.join(rel);
            if real.is_file() {
                reflink_or_copy(&real, &shadow)
                    .map_err(|e| StructuredDiagnostic::from_io("E_COW_COPY", &e, None))?;
            }
            self.mutations.insert(rel.to_path_buf(), MutationState::Modified);
        }
        Ok(shadow)
    }

    pub fn mark_whiteout(&mut self, rel: &Path) {
        let shadow = self.work_dir.join(rel);
        if shadow.is_file() {
            let _ = std::fs::remove_file(&shadow);
        }
        self.mutations.insert(rel.to_path_buf(), MutationState::Whiteout);
    }

    pub fn diff(&self) -> Vec<(String, &'static str)> {
        let mut out: Vec<(String, &'static str)> = self
            .mutations
            .iter()
            .map(|(p, st)| {
                let label = match st {
                    MutationState::Modified => "modified",
                    MutationState::Whiteout => "deleted",
                };
                (p.to_string_lossy().to_string(), label)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn merge(&mut self) -> Result<Vec<String>, StructuredDiagnostic> {
        let mut merged = Vec::new();
        let entries: Vec<(PathBuf, MutationState)> = self
            .mutations
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        for (rel, st) in entries {
            let dst = self.root.join(&rel);
            match st {
                MutationState::Modified => {
                    let src = self.work_dir.join(&rel);
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| StructuredDiagnostic::from_io("E_COW_MERGE", &e, None))?;
                    }
                    if std::fs::rename(&src, &dst).is_err() {
                        std::fs::copy(&src, &dst)
                            .map_err(|e| StructuredDiagnostic::from_io("E_COW_MERGE", &e, None))?;
                        let _ = std::fs::remove_file(&src);
                    }
                }
                MutationState::Whiteout => {
                    if dst.is_file() {
                        std::fs::remove_file(&dst)
                            .map_err(|e| StructuredDiagnostic::from_io("E_COW_MERGE", &e, None))?;
                    }
                }
            }
            merged.push(rel.to_string_lossy().to_string());
        }
        self.mutations.clear();
        Ok(merged)
    }

    pub fn sweep_orphans(root: &Path) -> usize {
        let sessions_dir = root.join(".koa").join("sessions");
        let Ok(entries) = std::fs::read_dir(&sessions_dir) else {
            return 0;
        };
        let mut reaped = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let lock_path = path.join("lease.lock");
            let can_reap = match OpenOptions::new().read(true).write(true).open(&lock_path) {
                Ok(file) => {
                    let rc = unsafe {
                        nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB)
                    };
                    rc == 0
                }
                Err(_) => true,
            };
            if can_reap && std::fs::remove_dir_all(&path).is_ok() {
                reaped += 1;
            }
        }
        reaped
    }

    pub fn sweep_stale_spills(max_age: Duration) -> usize {
        let tmp = std::env::temp_dir();
        let Ok(entries) = std::fs::read_dir(&tmp) else {
            return 0;
        };
        let now = SystemTime::now();
        let mut reaped = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("koa-spill-") {
                continue;
            }
            if let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
                && let Ok(age) = now.duration_since(modified)
                && age > max_age
                && std::fs::remove_file(entry.path()).is_ok()
            {
                reaped += 1;
            }
        }
        reaped
    }
}

impl Drop for CowWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.session_dir);
    }
}

fn reflink_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let s = File::open(src)?;
    let d = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dst)?;
    let rc = unsafe { nix::libc::ioctl(d.as_raw_fd(), FICLONE, s.as_raw_fd()) };
    if rc == 0 {
        return Ok(());
    }
    drop(d);
    drop(s);
    std::fs::copy(src, dst)?;
    Ok(())
}
