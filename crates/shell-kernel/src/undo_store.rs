use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use crate::fs_util::{Lease, LeaseError, reflink, state_dir};

#[derive(Debug)]
pub struct Snapshot {
    path: PathBuf,
    copied: u64,
}

impl Snapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn copied_bytes(&self) -> u64 {
        self.copied
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(crate) enum SnapshotError {
    OverBudget,
    Io(io::Error),
}

pub(crate) struct UndoStore {
    root: PathBuf,
    session_id: String,
    next: u64,
    open: Option<OpenStore>,
}

struct OpenStore {
    dir: PathBuf,
    _lease: Lease,
}

impl UndoStore {
    pub(crate) fn new(root: PathBuf, session_id: String) -> Self {
        Self {
            root,
            session_id,
            next: 0,
            open: None,
        }
    }

    pub(crate) fn snapshot(
        &mut self,
        src: &Path,
        len: u64,
        copy_budget: u64,
    ) -> Result<Snapshot, SnapshotError> {
        self.next += 1;
        let name = format!("snap-{}", self.next);
        let mut snap = Snapshot {
            path: self.dir().map_err(SnapshotError::Io)?.join(name),
            copied: 0,
        };
        if reflink(src, &snap.path).is_ok() {
            return Ok(snap);
        }
        if len > copy_budget {
            return Err(SnapshotError::OverBudget);
        }
        snap.copied = std::fs::copy(src, &snap.path).map_err(SnapshotError::Io)?;
        Ok(snap)
    }

    fn dir(&mut self) -> io::Result<&Path> {
        let open = match &mut self.open {
            Some(open) => open,
            slot @ None => slot.insert(OpenStore::create(&self.root, &self.session_id)?),
        };
        Ok(&open.dir)
    }
}

impl OpenStore {
    fn create(root: &Path, session_id: &str) -> io::Result<Self> {
        let dir = state_dir(root)?.join("undo").join(session_id);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        let lease = Lease::acquire(&dir).map_err(|e| match e {
            LeaseError::Io(e) => e,
            LeaseError::Held => io::Error::other("undo store is leased by another process"),
        })?;
        Ok(Self { dir, _lease: lease })
    }
}

impl Drop for UndoStore {
    fn drop(&mut self) {
        if let Some(open) = &self.open {
            let _ = std::fs::remove_dir_all(&open.dir);
        }
    }
}
