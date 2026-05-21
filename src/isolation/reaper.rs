//! Live build-subprocess registry so a single Ctrl-C tears the whole build
//! tree down at once.
//!
//! Build commands run either in their own process group (direct-exec path,
//! via `setpgid(0,0)`) or in a separate PID namespace (sandboxed path).  In
//! both cases the terminal's SIGINT never reaches them — it only goes to
//! wright's foreground process group — and the thread that launched them is
//! parked in `waitpid`.  Cancellation therefore has to signal each child
//! explicitly.
//!
//! Every [`run_in_isolation`](super::native::run_in_isolation) call registers
//! its child here while it runs and removes it on exit; [`cancel_all`] flips
//! the cancel flag and kills every registered child, which unblocks the
//! waiting threads so the cooperative cancel can roll back and exit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

/// True once the user has requested cancellation.  Checked by
/// [`is_cancelled`] so no fresh build starts after Ctrl-C.
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Monotonic id source for registry entries, so a guard removes exactly its
/// own child and never a later one that recycled the same PID.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Live build children awaiting reaping.
static ACTIVE: LazyLock<Mutex<Vec<Child>>> = LazyLock::new(|| Mutex::new(Vec::new()));

#[derive(Clone, Copy)]
struct Child {
    id: u64,
    pid: i32,
    /// `true` → signal the whole process group (`kill(-pid)`) to reap the
    /// child's `make`/`gcc` descendants (direct-exec path).  `false` →
    /// signal just `pid`; that child's `PR_SET_PDEATHSIG` then tears down
    /// the PID namespace it leads (sandboxed path).
    kill_pgroup: bool,
}

/// True if cancellation has been requested.  New isolation runs check this at
/// entry and refuse to start, so no compile is launched after Ctrl-C.
pub fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

/// Register a live child; the returned guard deregisters it on drop.
#[must_use]
pub fn register(pid: u32, kill_pgroup: bool) -> ChildGuard {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut active) = ACTIVE.lock() {
        active.push(Child {
            id,
            pid: pid as i32,
            kill_pgroup,
        });
    }
    ChildGuard { id }
}

/// Flip the cancel flag and SIGKILL every registered child (its whole process
/// group where applicable).  Idempotent and safe to call from a signal
/// handler context.
pub fn cancel_all() {
    CANCELLED.store(true, Ordering::SeqCst);
    if let Ok(active) = ACTIVE.lock() {
        for child in active.iter() {
            let target = if child.kill_pgroup {
                -child.pid
            } else {
                child.pid
            };
            // SIGKILL guarantees the build tree stops; these artefacts are
            // scratch and get rolled back, so graceful shutdown buys nothing.
            unsafe {
                libc::kill(target, libc::SIGKILL);
            }
        }
    }
}

/// Removes its child from the registry when dropped, i.e. once the child has
/// been reaped by `run_in_isolation`.
pub struct ChildGuard {
    id: u64,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.lock() {
            active.retain(|c| c.id != self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_count() -> usize {
        ACTIVE.lock().unwrap().len()
    }

    #[test]
    fn guard_registers_and_deregisters() {
        let before = active_count();
        {
            // PIDs here are never signalled — cancel_all is not called — so a
            // bogus value is safe and keeps the test from touching real
            // processes or the global cancel flag.
            let _g1 = register(u32::MAX, true);
            let _g2 = register(u32::MAX - 1, false);
            assert_eq!(active_count(), before + 2);
        }
        // Both guards dropped: registry is back to its starting size.
        assert_eq!(active_count(), before);
    }
}
