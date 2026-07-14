//! Compatibility facade for the former isolation-owned cancellation module.
//!
//! New engine code uses `crate::cancellation`; cancellation supervises direct
//! execution and non-isolation build phases as well as namespace children.

pub use crate::cancellation::ChildGuard;

pub fn is_cancelled() -> bool {
    crate::cancellation::is_cancelled()
}

pub fn register(pid: u32, kill_pgroup: bool) -> ChildGuard {
    crate::cancellation::register(pid, kill_pgroup)
}

pub fn cancel_all() {
    crate::cancellation::cancel_all();
}

pub fn spawn_signal_handler(cancel_tx: tokio::sync::watch::Sender<bool>, quiet: bool) {
    crate::cancellation::spawn_signal_handler(cancel_tx, quiet);
}
