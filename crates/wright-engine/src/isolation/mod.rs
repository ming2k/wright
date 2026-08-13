mod direct;
mod error;
mod helper;
#[doc(hidden)]
pub mod native;
mod process;
#[doc(hidden)]
mod resources;

use std::io::{Read, Seek, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::process::ExitStatus;

use crate::error::Result;

pub use error::IsolationError;
pub use wright_model::isolation::IsolationLevel;

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
    /// Override the executable used for the single-threaded isolation helper.
    /// Normal Wright invocations use the current executable. Embedders and
    /// integration tests may point this at a compatible Wright binary.
    #[doc(hidden)]
    pub helper_executable: Option<PathBuf>,
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
            helper_executable: None,
        }
    }

    /// Validate every path that will be interpreted after entering a mount
    /// namespace. This runs in the parent before any fork or filesystem
    /// cleanup, so malformed paths cannot escape the task scratch directory.
    pub fn validate(&self) -> std::result::Result<(), IsolationError> {
        validate_task_id(&self.task_id)?;
        self.rlimits.prepare()?;

        if self.cpu_count == Some(0) {
            return Err(IsolationError::InvalidConfig(
                "cpu count must be greater than zero".to_string(),
            ));
        }

        for (key, value) in &self.env {
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(IsolationError::InvalidConfig(format!(
                    "environment variable name {key:?} is invalid"
                )));
            }
            if value.contains('\0') {
                return Err(IsolationError::InvalidConfig(format!(
                    "environment variable {key:?} contains a NUL byte"
                )));
            }
        }

        if self.level == IsolationLevel::None {
            return Ok(());
        }

        for (label, path) in [
            ("base root", self.base_root.as_path()),
            ("source directory", self.src_dir.as_path()),
            ("output directory", self.output_dir.as_path()),
        ] {
            validate_absolute_directory(label, path)?;
        }

        let build_root = self.src_dir.parent().unwrap_or(self.src_dir.as_path());
        for (label, path) in [
            ("base root", self.base_root.as_path()),
            ("build root", build_root),
        ] {
            validate_overlay_option_path(label, path)?;
        }

        for (source, target, _) in &self.extra_binds {
            validate_mount_target("extra bind", target)?;
            validate_mount_source("extra bind", source)?;
        }
        for (source, target) in &self.dep_mounts {
            validate_mount_target("dependency bind", target)?;
            validate_mount_source("dependency bind", source)?;
        }

        if let Some(helper) = &self.helper_executable {
            if !helper.is_absolute() {
                return Err(IsolationError::InvalidConfig(format!(
                    "isolation helper executable must be absolute: {}",
                    helper.display()
                )));
            }
            if !helper.is_file() {
                return Err(IsolationError::InvalidConfig(format!(
                    "isolation helper executable is not a file: {}",
                    helper.display()
                )));
            }
        }

        Ok(())
    }
}

fn validate_task_id(task_id: &str) -> std::result::Result<(), IsolationError> {
    let mut components = Path::new(task_id).components();
    let is_single_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    let has_reserved_mount_char = task_id
        .bytes()
        .any(|byte| matches!(byte, b',' | b':' | b'\\'));

    if !is_single_component || has_reserved_mount_char || task_id.len() > 160 {
        return Err(IsolationError::InvalidConfig(format!(
            "task id {task_id:?} must be at most 160 bytes and one path component without ',', ':', or '\\'"
        )));
    }
    Ok(())
}

fn validate_absolute_directory(
    label: &str,
    path: &Path,
) -> std::result::Result<(), IsolationError> {
    if !path.is_absolute() {
        return Err(IsolationError::InvalidConfig(format!(
            "{label} must be absolute: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(IsolationError::InvalidConfig(format!(
            "{label} is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_overlay_option_path(
    label: &str,
    path: &Path,
) -> std::result::Result<(), IsolationError> {
    if path
        .as_os_str()
        .as_bytes()
        .iter()
        .any(|byte| matches!(byte, b',' | b':' | b'\\'))
    {
        return Err(IsolationError::InvalidConfig(format!(
            "{label} contains a character reserved by OverlayFS mount options: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_mount_source(kind: &str, source: &Path) -> std::result::Result<(), IsolationError> {
    if !source.is_absolute() {
        return Err(IsolationError::InvalidConfig(format!(
            "{kind} source must be absolute: {}",
            source.display()
        )));
    }
    std::fs::metadata(source).map_err(|error| {
        IsolationError::InvalidConfig(format!(
            "{kind} source does not exist or is inaccessible: {}: {error}",
            source.display()
        ))
    })?;
    Ok(())
}

fn validate_mount_target(kind: &str, target: &Path) -> std::result::Result<(), IsolationError> {
    let mut components = target.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(IsolationError::InvalidConfig(format!(
            "{kind} target must be absolute: {}",
            target.display()
        )));
    }

    let mut saw_normal = false;
    for component in components {
        match component {
            Component::Normal(name) => {
                if !saw_normal && name == ".old_root" {
                    return Err(IsolationError::InvalidConfig(format!(
                        "{kind} target uses reserved isolation path: {}",
                        target.display()
                    )));
                }
                saw_normal = true;
            }
            Component::RootDir => {}
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                return Err(IsolationError::InvalidConfig(format!(
                    "{kind} target contains a traversal component: {}",
                    target.display()
                )));
            }
        }
    }

    if !saw_normal {
        return Err(IsolationError::InvalidConfig(format!(
            "{kind} target cannot be the isolation root"
        )));
    }
    Ok(())
}

pub fn run_in_isolation(
    config: &mut IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<IsolationOutput> {
    let result = (|| {
        config.validate()?;
        if crate::cancellation::is_cancelled() {
            return Err(IsolationError::Cancelled);
        }

        match config.level {
            IsolationLevel::None => direct::run(config, command, args),
            IsolationLevel::Relaxed | IsolationLevel::Strict => {
                helper::run_parent(config, command, args)
            }
        }
    })();
    result.map_err(Into::into)
}

/// Run the internal single-threaded isolation helper protocol.
///
/// This is called by the `wright` binary before it creates a Tokio runtime.
#[doc(hidden)]
pub fn run_helper_process() -> ! {
    helper::run_helper_process()
}

/// Return whether the current process was started as Wright's internal
/// isolation helper. Binaries must check this before creating worker threads.
#[doc(hidden)]
pub fn is_helper_process() -> bool {
    helper::is_helper_process()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> (tempfile::TempDir, tempfile::TempDir, IsolationConfig) {
        let src = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let config = IsolationConfig::new(
            IsolationLevel::Strict,
            src.path().to_path_buf(),
            output.path().to_path_buf(),
            "task-1".to_string(),
        );
        (src, output, config)
    }

    #[test]
    fn validates_standard_isolation_config() {
        let (_src, _output, config) = valid_config();
        config.validate().unwrap();
    }

    #[test]
    fn rejects_task_id_path_traversal() {
        let (_src, _output, mut config) = valid_config();
        config.task_id = "../outside".to_string();

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("task id"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_mount_target_path_traversal() {
        let (_src, _output, mut config) = valid_config();
        config.dep_mounts.push((
            PathBuf::from("/tmp/dependency"),
            PathBuf::from("/usr/../outside"),
        ));

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("traversal"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_missing_mount_source() {
        let (_src, output, mut config) = valid_config();
        config.extra_binds.push((
            output.path().join("missing"),
            PathBuf::from("/required"),
            true,
        ));

        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("does not exist or is inaccessible"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_reserved_overlay_option_characters() {
        let (_src, _output, mut config) = valid_config();
        config.base_root = PathBuf::from("/tmp/root,upperdir=/tmp/escape");

        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("not a directory") || error.contains("reserved"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_memory_limit_overflow_before_execution() {
        let src = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut config = IsolationConfig::new(
            IsolationLevel::None,
            src.path().to_path_buf(),
            output.path().to_path_buf(),
            "resource-overflow".to_string(),
        );
        config.rlimits.memory_mb = Some(u64::MAX);

        let error = config.validate().unwrap_err().to_string();
        assert!(error.contains("overflows"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_zero_cpu_count() {
        let (_src, _output, mut config) = valid_config();
        config.cpu_count = Some(0);

        let error = config.validate().unwrap_err().to_string();
        assert!(
            error.contains("greater than zero"),
            "unexpected error: {error}"
        );
    }
}
