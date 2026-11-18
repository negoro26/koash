use nix::libc;
use std::sync::atomic::{AtomicI32, Ordering};

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

const SLOTS: usize = 64;

static GROUPS: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];

pub(crate) fn publish(pgids: impl Iterator<Item = i32>) {
    let mut pgids = pgids;
    for slot in &GROUPS {
        slot.store(pgids.next().unwrap_or(0), Ordering::Relaxed);
    }
}

extern "C" fn forward(sig: libc::c_int) {
    let to_jobs = if sig == libc::SIGINT {
        libc::SIGHUP
    } else {
        sig
    };
    for slot in &GROUPS {
        let pgid = slot.load(Ordering::Relaxed);
        if pgid > 0 {
            unsafe {
                libc::killpg(pgid, to_jobs);
                libc::killpg(pgid, libc::SIGCONT);
            }
        }
    }
    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

pub fn forward_to_jobs(interactive: bool) {
    let action = SigAction::new(
        SigHandler::Handler(forward),
        SaFlags::empty(),
        SigSet::empty(),
    );
    let leader = nix::unistd::getsid(None).ok() == Some(nix::unistd::getpid());
    let signals: &[Signal] = match (interactive, leader) {
        (true, _) => &[Signal::SIGHUP],
        (false, false) => &[Signal::SIGHUP, Signal::SIGTERM],
        (false, true) => &[Signal::SIGHUP, Signal::SIGTERM, Signal::SIGINT],
    };
    for &sig in signals {
        let _ = unsafe { sigaction(sig, &action) };
    }
}
