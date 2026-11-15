use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const FICLONE: nix::libc::c_ulong = 0x4004_9409;

/// A directory with no lease file this young may still be under construction by its owner.
const LEASE_GRACE: Duration = Duration::from_secs(60);

pub(crate) fn reflink(src: &Path, dst: &Path) -> io::Result<()> {
    let s = File::open(src)?;
    let d = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dst)?;
    let rc = unsafe { nix::libc::ioctl(d.as_raw_fd(), FICLONE, s.as_raw_fd()) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn reflink_or_copy(src: &Path, dst: &Path) -> io::Result<()> {
    if reflink(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    Ok(())
}

pub(crate) enum LeaseError {
    Io(io::Error),
    Held,
}

/// The kernel drops the lock when the process dies, which is how the sweep tells a crashed owner from a live one.
pub(crate) struct Lease {
    _file: File,
}

impl Lease {
    pub(crate) fn acquire(dir: &Path) -> Result<Self, LeaseError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("lease.lock"))
            .map_err(LeaseError::Io)?;
        if try_lock_exclusive(&file) {
            Ok(Self { _file: file })
        } else {
            Err(LeaseError::Held)
        }
    }
}

fn try_lock_exclusive(file: &File) -> bool {
    let rc = unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    rc == 0
}

/// Never follows symlinks: a checkout that ships `.koa/undo` as a link must not turn the sweep into a delete of its target.
pub(crate) fn sweep_unleased_dirs(parent: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut reaped = 0usize;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        let can_reap = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join("lease.lock"))
        {
            Ok(file) => try_lock_exclusive(&file),
            Err(_) => is_older_than(&path, LEASE_GRACE),
        };
        if can_reap && std::fs::remove_dir_all(&path).is_ok() {
            reaped += 1;
        }
    }
    reaped
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

pub(crate) fn state_subdir(root: &Path, child: &str) -> Option<PathBuf> {
    let state = root.join(".koa");
    let dir = state.join(child);
    (is_real_dir(&state) && is_real_dir(&dir)).then_some(dir)
}

/// The `.gitignore` keeps undo snapshots and session copies out of `git add -A`.
pub(crate) fn state_dir(root: &Path) -> io::Result<PathBuf> {
    let dir = root.join(".koa");
    std::fs::create_dir_all(&dir)?;
    if !is_real_dir(&dir) {
        return Err(io::Error::other(format!(
            "{} is not a real directory",
            dir.display()
        )));
    }
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")?;
    }
    Ok(dir)
}

fn is_older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed > age)
}
