pub mod native;

use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::process::ExitStatus;

use crate::error::Result;

/// Captured subprocess output, streamed to a temporary file with only the
/// tail kept in memory for error display.
pub struct CapturedOutput {
    /// Temporary file containing the full output, seeked to the beginning.
    pub file: std::fs::File,
    /// Last ~16 KB of output for error display without re-reading the file.
    pub tail: String,
}

/// Captured output from a isolation command execution.
pub struct IsolationOutput {
    pub status: ExitStatus,
    pub stdout: CapturedOutput,
    pub stderr: CapturedOutput,
}

const TAIL_BYTES: u64 = 16384;

/// Spawn a thread that reads from `source` in 8 KB chunks, streams to
/// `dest` file, optionally echoes to the terminal and/or a log sink, and
/// keeps the last [`TAIL_BYTES`] for error display.  Returns a
/// [`CapturedOutput`] with the file seeked to the beginning ready for the
/// caller to read.
pub fn spawn_stream_reader<R: Read + Send + 'static>(
    source: R,
    mut echo_to: Option<Box<dyn Write + Send>>,
    mut log_to: Option<Box<dyn Write + Send>>,
    mut dest: std::fs::File,
) -> std::thread::JoinHandle<CapturedOutput> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut total: u64 = 0;
        let mut source = source;
        loop {
            match source.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = dest.write_all(&buf[..n]);
                    if let Some(ref mut w) = echo_to {
                        let _ = w.write_all(&buf[..n]);
                        let _ = w.flush();
                    }
                    if let Some(ref mut w) = log_to {
                        let _ = w.write_all(&buf[..n]);
                        let _ = w.flush();
                    }
                    total += n as u64;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }

        // Extract tail
        let tail = if total > 0 {
            let tail_start = total.saturating_sub(TAIL_BYTES);
            let _ = dest.seek(std::io::SeekFrom::Start(tail_start));
            let mut tail_buf = Vec::with_capacity((total - tail_start) as usize);
            let _ = dest.read_to_end(&mut tail_buf);
            String::from_utf8_lossy(&tail_buf).into_owned()
        } else {
            String::new()
        };

        // Seek to beginning so the caller can stream the full content
        let _ = dest.seek(std::io::SeekFrom::Start(0));

        CapturedOutput { file: dest, tail }
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
pub enum IsolationLevel {
    None,
    Relaxed,
    Strict,
}

impl std::str::FromStr for IsolationLevel {
    type Err = crate::error::WrightError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "relax" | "relaxed" => Ok(Self::Relaxed),
            "strict" => Ok(Self::Strict),
            _ => Err(crate::error::WrightError::IsolationError(format!(
                "unknown isolation level: '{}' (valid: none, relaxed, strict)",
                s
            ))),
        }
    }
}

pub struct IsolationConfig {
    pub level: IsolationLevel,
    pub base_root: PathBuf,
    pub src_dir: PathBuf,
    pub output_dir: PathBuf,
    pub task_id: String, // Unique identifier for this build task
    pub extra_binds: Vec<(PathBuf, PathBuf, bool)>, // (host_path, dest_path, read_only)
    pub env: Vec<(String, String)>,
    pub rlimits: ResourceLimits,
    pub verbose: bool, // Whether to echo subprocess output to the terminal
    /// Pin the isolation process to this many CPUs via sched_setaffinity.
    /// Tools like `nproc` will then return this count naturally without any
    /// env var injection. None means inherit the host's full CPU set.
    pub cpu_count: Option<u32>,
    /// When set, subprocess stdout is tee'd to this file in real time.
    pub log_stdout: Option<std::fs::File>,
    /// When set, subprocess stderr is tee'd to this file in real time.
    pub log_stderr: Option<std::fs::File>,
    /// Build-dependency mounts: (host_path, isolation_path).
    /// These are mounted read-only into the isolation environment.
    pub dep_mounts: Vec<(PathBuf, PathBuf)>,
}

impl IsolationConfig {
    pub fn new(
        level: IsolationLevel,
        src_dir: PathBuf,
        output_dir: PathBuf,
        task_id: String,
    ) -> Self {
        Self {
            level,
            base_root: PathBuf::from("/"),
            src_dir,
            output_dir,
            task_id,
            extra_binds: Vec::new(),
            env: Vec::new(),
            rlimits: ResourceLimits::default(),
            verbose: false,
            cpu_count: None,
            log_stdout: None,
            log_stderr: None,
            dep_mounts: Vec::new(),
        }
    }
}

/// Run a command inside a isolation using the native Linux namespace implementation.
pub fn run_in_isolation(
    config: &mut IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<IsolationOutput> {
    native::run_in_isolation(config, command, args)
}
