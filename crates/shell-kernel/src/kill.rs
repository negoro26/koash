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

/// Groups stay tracked until the output readers finish, not just the direct children: surviving members hold
/// the readers open. Linux never reuses a pid while it is a live pgid, so `killpg` cannot hit a stranger.
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
