//! Filesystem primitives shared by the CoW workspace and the undo store: block-sharing
//! file clones and the `flock` lease that marks a state directory as owned by a live process.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, SystemTime};

const FICLONE: nix::libc::c_ulong = 0x4004_9409;

/// A directory with no lease file this young may still be under construction by its owner.
const LEASE_GRACE: Duration = Duration::from_secs(60);

/// Shares `src`'s blocks with `dst` instead of copying them. Fails where the filesystem
/// cannot clone, such as tmpfs or across mount points. `dst` is created or truncated first.
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

/// An exclusive `flock` on `<dir>/lease.lock`. The kernel drops it when the process dies,
/// which is how [`sweep_unleased_dirs`] tells a crashed owner from a live one.
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

/// Removes every child directory of `parent` whose owner is gone and returns how many it
/// removed. A child without a lease file is left alone while it is younger than
/// [`LEASE_GRACE`], because its owner creates the directory before it takes the lease.
pub(crate) fn sweep_unleased_dirs(parent: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut reaped = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
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

fn is_older_than(path: &Path, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed > age)
}
