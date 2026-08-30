//! Child-process reaping coordination.
//!
//! Two parties care about sandbox children:
//!   * each [`ProcessSupervisor`] needs its own child's exit status **and** `rusage`
//!     (accurate exit code / CPU time / peak memory → correct TLE/MLE/RE verdicts);
//!   * orphaned grandchildren (helpers a sandboxed program forked and left behind) must
//!     be reaped so the container never accumulates zombies.
//!
//! If both a global `SIGCHLD` handler and the supervisor call `wait*()`, they race: the
//! handler can steal a supervised child's status, leaving the supervisor with `ECHILD`
//! and a fabricated `exit 0 / 0 cpu` result.
//!
//! Design: a **single** reaper ([`reap_all`], driven by `SIGCHLD`) is the only `wait4`
//! caller. It reaps every terminated child; for a *supervised* pid it stashes the
//! `(status, rusage)` in that pid's slot for the supervisor to pick up, and for an
//! *orphan* it simply discards the status. This removes the race entirely and, unlike the
//! previous `WNOWAIT`-peek approach, never lets a supervised zombie at the front starve
//! orphan reaping.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Exit information the reaper collects for a supervised child.
pub type ChildExit = (libc::c_int, libc::rusage);

type Slot = Arc<Mutex<Option<ChildExit>>>;

fn registry() -> &'static Mutex<HashMap<i32, Slot>> {
    static R: OnceLock<Mutex<HashMap<i32, Slot>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// RAII handle held by a `ProcessSupervisor` for the lifetime of one child. While it is
/// alive the reaper routes that child's exit into `slot`; dropping it unregisters the pid
/// (any not-yet-collected exit is then simply discarded on the next `reap_all`).
pub struct SupervisedChild {
    pid: i32,
    slot: Slot,
}

/// Register `pid` as supervised. Call immediately after fork, in the parent, before the
/// child can exit — otherwise the reaper may reap it before the slot exists.
pub fn register_child(pid: i32) -> SupervisedChild {
    let slot: Slot = Arc::new(Mutex::new(None));
    registry()
        .lock()
        .expect("reaper registry poisoned")
        .insert(pid, slot.clone());
    SupervisedChild { pid, slot }
}

impl SupervisedChild {
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Non-blocking: returns the child's `(status, rusage)` once the reaper has collected
    /// it, else `None`. The supervisor polls this asynchronously.
    pub fn try_take(&self) -> Option<ChildExit> {
        self.slot.lock().expect("reaper slot poisoned").take()
    }
}

impl Drop for SupervisedChild {
    fn drop(&mut self) {
        registry()
            .lock()
            .expect("reaper registry poisoned")
            .remove(&self.pid);
    }
}

/// Reap every terminated child. Supervised children's exit info is stashed for their
/// supervisor; orphans are consumed and discarded. Being the sole `wait4` caller, this
/// never races a supervisor. Intended to be called from the `SIGCHLD` handler.
#[cfg(target_os = "linux")]
pub fn reap_all() {
    loop {
        let mut status: libc::c_int = 0;
        let mut rusage: libc::rusage = unsafe { std::mem::zeroed() };
        let pid = unsafe { libc::wait4(-1, &mut status, libc::WNOHANG, &mut rusage) };
        if pid <= 0 {
            break; // no more terminated children right now
        }
        let slot = registry()
            .lock()
            .expect("reaper registry poisoned")
            .get(&pid)
            .cloned();
        if let Some(slot) = slot {
            *slot.lock().expect("reaper slot poisoned") = Some((status, rusage));
        }
        // else: orphan grandchild — reaped above; nothing waiting for it, so discard.
    }
}
