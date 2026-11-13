use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::Pid;

#[derive(Debug)]
struct Group {
    pgid: Pid,
    live: usize,
}

#[derive(Debug, Default)]
struct KillState {
    killed: AtomicBool,
    groups: Mutex<Vec<Group>>,
}

/// Cancels a running program: sets a flag the walk loops poll and SIGKILLs every tracked process group.
///
/// A group stays tracked until its last member is reaped, and both steps happen under one mutex,
/// so `kill` can never signal a recycled pgid.
#[derive(Debug, Clone, Default)]
pub struct KillSwitch(Arc<KillState>);

impl KillSwitch {
    pub fn is_killed(&self) -> bool {
        self.0.killed.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.0.killed.store(false, Ordering::SeqCst);
    }

    pub fn kill(&self) {
        let groups = self.lock();
        self.0.killed.store(true, Ordering::SeqCst);
        for group in groups.iter() {
            let _ = killpg(group.pgid, Signal::SIGKILL);
        }
    }

    /// Registers one more live member of `pgid`; kills it at once when the switch already fired.
    pub fn track(&self, pgid: Pid) {
        let mut groups = self.lock();
        match groups.iter_mut().find(|g| g.pgid == pgid) {
            Some(group) => group.live += 1,
            None => groups.push(Group { pgid, live: 1 }),
        }
        if self.is_killed() {
            let _ = killpg(pgid, Signal::SIGKILL);
        }
    }

    pub fn untrack(&self, pgid: Pid) {
        self.lock().retain(|g| g.pgid != pgid);
    }

    /// Next wait event of `pgid`. Blocks until one is available unless `block` is false,
    /// in which case `Ok(None)` means nothing happened yet.
    pub fn wait_group(&self, pgid: Pid, block: bool) -> nix::Result<Option<WaitStatus>> {
        if block {
            // Peek without reaping so the zombie keeps the pgid reserved until we hold the lock.
            waitid(
                Id::PGid(pgid),
                WaitPidFlag::WEXITED | WaitPidFlag::WSTOPPED | WaitPidFlag::WNOWAIT,
            )?;
        }
        let mut groups = self.lock();
        let status = waitpid(
            Pid::from_raw(-pgid.as_raw()),
            Some(WaitPidFlag::WUNTRACED | WaitPidFlag::WNOHANG),
        )?;
        match status {
            WaitStatus::StillAlive => return Ok(None),
            WaitStatus::Exited(..) | WaitStatus::Signaled(..) => {
                if let Some(pos) = groups.iter().position(|g| g.pgid == pgid) {
                    groups[pos].live = groups[pos].live.saturating_sub(1);
                    if groups[pos].live == 0 {
                        groups.swap_remove(pos);
                    }
                }
            }
            _ => {}
        }
        Ok(Some(status))
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Group>> {
        self.0
            .groups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
