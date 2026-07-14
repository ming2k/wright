//! Process-wide cancellation and live build-subprocess supervision.
//!
//! Build commands run either in their own process group or in a separate PID
//! namespace. In both cases Wright must explicitly terminate them when the
//! foreground process receives SIGINT or SIGTERM.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

static CANCELLED: AtomicBool = AtomicBool::new(false);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ACTIVE: LazyLock<Mutex<Vec<Child>>> = LazyLock::new(|| Mutex::new(Vec::new()));

#[derive(Clone, Copy)]
struct Child {
    id: u64,
    pid: i32,
    /// `true` signals the entire process group; `false` signals a PID-
    /// namespace supervisor whose parent-death signal terminates its init.
    kill_pgroup: bool,
}

pub(crate) fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

#[must_use]
pub struct ChildGuard {
    id: u64,
}

fn active_children() -> MutexGuard<'static, Vec<Child>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn terminate(child: Child) {
    let target = if child.kill_pgroup {
        -child.pid
    } else {
        child.pid
    };
    // SAFETY: `target` is either the registered child PID or the negative
    // process-group ID led by that child.
    unsafe {
        libc::kill(target, libc::SIGKILL);
    }
}

pub(crate) fn register(pid: u32, kill_pgroup: bool) -> ChildGuard {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let child = Child {
        id,
        pid: pid as i32,
        kill_pgroup,
    };
    let mut active = active_children();
    if CANCELLED.load(Ordering::SeqCst) {
        // Cancellation may race with process creation. If the signal handler
        // ran before this child reached the registry, terminate it here rather
        // than allowing it to escape the first cancellation pass.
        terminate(child);
    } else {
        active.push(child);
    }
    ChildGuard { id }
}

pub(crate) fn cancel_all() {
    CANCELLED.store(true, Ordering::SeqCst);
    let active = active_children();
    for child in active.iter().copied() {
        terminate(child);
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        active_children().retain(|child| child.id != self.id);
    }
}

/// Install the two-tier SIGINT/SIGTERM behavior for a build operation.
pub(crate) fn spawn_signal_handler(cancel_tx: tokio::sync::watch::Sender<bool>, quiet: bool) {
    tokio::spawn(async move {
        wait_for_signal().await;
        cancel_all();
        let _ = cancel_tx.send(true);
        if !quiet {
            eprintln!("\nInterrupting — press Ctrl-C again to force-quit.");
        }
        wait_for_signal().await;
        cancel_all();
        std::process::exit(130);
    });
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let ctrl_c = tokio::signal::ctrl_c();
        match signal(SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = ctrl_c => {},
                    _ = sigterm.recv() => {},
                }
            }
            Err(_) => {
                ctrl_c.await.ok();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_count() -> usize {
        active_children().len()
    }

    #[test]
    fn guard_registers_and_deregisters() {
        let before = active_count();
        {
            let _first = register(u32::MAX, true);
            let _second = register(u32::MAX - 1, false);
            assert_eq!(active_count(), before + 2);
        }
        assert_eq!(active_count(), before);
    }
}
