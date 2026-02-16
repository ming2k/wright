pub mod bwrap;
pub mod native;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::error::Result;

/// Captured output from a sandboxed command execution.
pub struct SandboxOutput {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLevel {
    None,
    Relaxed,
    Strict,
}

impl SandboxLevel {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "none" => Self::None,
            "relaxed" => Self::Relaxed,
            "strict" | _ => Self::Strict,
        }
    }
}

pub struct SandboxConfig {
    pub level: SandboxLevel,
    pub src_dir: PathBuf,
    pub pkg_dir: PathBuf,
    pub files_dir: Option<PathBuf>,
    pub extra_binds: Vec<(PathBuf, PathBuf, bool)>, // (host_path, dest_path, read_only)
    pub env: Vec<(String, String)>,
}

impl SandboxConfig {
    pub fn new(level: SandboxLevel, src_dir: PathBuf, pkg_dir: PathBuf) -> Self {
        Self {
            level,
            src_dir,
            pkg_dir,
            files_dir: None,
            extra_binds: Vec::new(),
            env: Vec::new(),
        }
    }
}

/// Run a command inside a sandbox using the native Linux namespace implementation.
pub fn run_in_sandbox(config: &SandboxConfig, command: &str, args: &[String]) -> Result<SandboxOutput> {
    native::run_in_sandbox(config, command, args)
}
