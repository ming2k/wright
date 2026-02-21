pub mod native;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::error::Result;

/// Captured output from a dockyard command execution.
pub struct DockyardOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Spawn a thread that reads from `source` in 8 KB chunks, echoes each chunk
/// to `echo_to` (for real-time terminal output), and accumulates the bytes.
/// Returns the accumulated output when EOF is reached.
pub fn spawn_tee_reader<R: Read + Send + 'static>(
    source: R,
    mut echo_to: impl Write + Send + 'static,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut accumulated = Vec::new();
        let mut source = source;
        loop {
            match source.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = echo_to.write_all(&buf[..n]);
                    let _ = echo_to.flush();
                    accumulated.extend_from_slice(&buf[..n]);
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        accumulated
    })
}

#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    /// RLIMIT_AS: max virtual address space in megabytes.
    /// Note: this limits virtual address space, not physical RSS.
    /// Set generously — programs like rustc/JVM/Go reserve large
    /// virtual mappings without touching them.
    pub memory_mb: Option<u64>,
    /// RLIMIT_CPU: max CPU time (user + system) in seconds.
    pub cpu_time_secs: Option<u64>,
    /// Wall-clock timeout in seconds (enforced by parent, not rlimit).
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockyardLevel {
    None,
    Relaxed,
    Strict,
}

impl std::str::FromStr for DockyardLevel {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "none" => Self::None,
            "relaxed" => Self::Relaxed,
            _ => Self::Strict,
        })
    }
}

pub struct DockyardConfig {
    pub level: DockyardLevel,
    pub src_dir: PathBuf,
    pub pkg_dir: PathBuf,
    pub task_id: String, // Unique identifier for this build task
    pub files_dir: Option<PathBuf>,
    pub extra_binds: Vec<(PathBuf, PathBuf, bool)>, // (host_path, dest_path, read_only)
    pub env: Vec<(String, String)>,
    pub rlimits: ResourceLimits,
    pub verbose: bool, // Whether to echo subprocess output to the terminal
    /// Pin the dockyard process to this many CPUs via sched_setaffinity.
    /// Tools like `nproc` will then return this count naturally without any
    /// env var injection. None means inherit the host's full CPU set.
    pub cpu_count: Option<u32>,
}

impl DockyardConfig {
    pub fn new(level: DockyardLevel, src_dir: PathBuf, pkg_dir: PathBuf, task_id: String) -> Self {
        Self {
            level,
            src_dir,
            pkg_dir,
            task_id,
            files_dir: None,
            extra_binds: Vec::new(),
            env: Vec::new(),
            rlimits: ResourceLimits::default(),
            verbose: false,
            cpu_count: None,
        }
    }
}

/// Run a command inside a dockyard using the native Linux namespace implementation.
pub fn run_in_dockyard(config: &DockyardConfig, command: &str, args: &[String]) -> Result<DockyardOutput> {
    native::run_in_dockyard(config, command, args)
}
