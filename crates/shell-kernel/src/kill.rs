use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use nix::sys::signal::{Signal, killpg};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;

#[derive(Debug, Default)]
struct KillState {
    killed: AtomicBool,
    groups: Mutex<Vec<Pid>>,
}

/// Cancels a running program: sets a flag the walk loops poll and SIGKILLs every tracked process group.
///
/// A job's group stays tracked until the whole pipeline is done, which includes its output readers,
/// not just its direct children. Members that outlive the children keep the readers blocked, and
/// `kill` is the only way to reach them. Linux does not hand a pid out again while it is in use as
/// a pgid, so `killpg` stays safe for as long as any member lives.
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
        for &pgid in groups.iter() {
            let _ = killpg(pgid, Signal::SIGKILL);
        }
    }

    /// Registers `pgid`; kills it at once when the switch already fired.
    pub fn track(&self, pgid: Pid) {
        let mut groups = self.lock();
        if !groups.contains(&pgid) {
            groups.push(pgid);
        }
        if self.is_killed() {
            let _ = killpg(pgid, Signal::SIGKILL);
        }
    }

    pub fn untrack(&self, pgid: Pid) {
        self.lock().retain(|&g| g != pgid);
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Pid>> {
        self.0
            .groups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Next wait event of a direct child in `pgid`, reporting stops as well. Blocks until one is
/// available unless `block` is false, in which case `Ok(None)` means nothing happened yet.
pub fn wait_group(pgid: Pid, block: bool) -> nix::Result<Option<WaitStatus>> {
    let mut flags = WaitPidFlag::WUNTRACED;
    if !block {
        flags |= WaitPidFlag::WNOHANG;
    }
    match waitpid(Pid::from_raw(-pgid.as_raw()), Some(flags))? {
        WaitStatus::StillAlive => Ok(None),
        status => Ok(Some(status)),
    }
}
