//! Single-threaded process boundary for Linux namespace setup.
//!
//! The application process only spawns a fresh copy of the Wright executable.
//! That copy decodes this protocol before creating a Tokio runtime or any
//! threads, then delegates the fork/unshare/mount sequence to `native`.

use std::ffi::{OsStr, OsString};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Stdio, exit};

use serde::{Deserialize, Serialize};

use super::error::{IsolationError, Result};
use super::{IsolationConfig, IsolationLevel, IsolationOutput, ResourceLimits};

const PROTOCOL_VERSION: u8 = 3;
const INTERNAL_ARGUMENT: &str = "__wright_isolation_helper_v2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WirePath(Vec<u8>);

impl WirePath {
    fn from_path(path: &Path) -> Self {
        Self(path.as_os_str().as_bytes().to_vec())
    }

    fn into_path(self) -> PathBuf {
        PathBuf::from(OsString::from_vec(self.0))
    }
}

#[derive(Serialize, Deserialize)]
struct HelperRequest {
    version: u8,
    status_path: WirePath,
    level: String,
    base_root: WirePath,
    src_dir: WirePath,
    output_dir: WirePath,
    task_id: String,
    extra_binds: Vec<(WirePath, WirePath, bool)>,
    env: Vec<(String, String)>,
    memory_mb: Option<u64>,
    cpu_time_secs: Option<u64>,
    // Wall-clock timeout is deliberately absent: the application supervises
    // the helper process and owns that policy.
    cpu_count: Option<u32>,
    dep_mounts: Vec<(WirePath, WirePath)>,
    stage_overlay: Option<(WirePath, WirePath, WirePath)>,
    command: String,
    args: Vec<String>,
}

impl HelperRequest {
    fn new(config: &IsolationConfig, status_path: &Path, command: &str, args: &[String]) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            status_path: WirePath::from_path(status_path),
            level: config.level.to_string(),
            base_root: WirePath::from_path(&config.base_root),
            src_dir: WirePath::from_path(&config.src_dir),
            output_dir: WirePath::from_path(&config.output_dir),
            task_id: config.task_id.clone(),
            extra_binds: config
                .extra_binds
                .iter()
                .map(|(source, target, read_only)| {
                    (
                        WirePath::from_path(source),
                        WirePath::from_path(target),
                        *read_only,
                    )
                })
                .collect(),
            env: config.env.clone(),
            memory_mb: config.rlimits.memory_mb,
            cpu_time_secs: config.rlimits.cpu_time_secs,
            cpu_count: config.cpu_count,
            dep_mounts: config
                .dep_mounts
                .iter()
                .map(|(source, target)| (WirePath::from_path(source), WirePath::from_path(target)))
                .collect(),
            stage_overlay: config.stage_overlay.as_ref().map(|overlay| {
                (
                    WirePath::from_path(&overlay.lowerdir),
                    WirePath::from_path(&overlay.upperdir),
                    WirePath::from_path(&overlay.workdir),
                )
            }),
            command: command.to_string(),
            args: args.to_vec(),
        }
    }

    fn into_parts(self) -> Result<(PathBuf, IsolationConfig, String, Vec<String>)> {
        if self.version != PROTOCOL_VERSION {
            return Err(IsolationError::InvalidConfig(format!(
                "unsupported isolation helper protocol version {}",
                self.version
            )));
        }
        let level = self.level.parse::<IsolationLevel>().map_err(|error| {
            IsolationError::InvalidConfig(format!("invalid helper isolation level: {error}"))
        })?;
        let mut config = IsolationConfig::new(
            level,
            self.src_dir.into_path(),
            self.output_dir.into_path(),
            self.task_id,
        );
        config.base_root = self.base_root.into_path();
        config.extra_binds = self
            .extra_binds
            .into_iter()
            .map(|(source, target, read_only)| (source.into_path(), target.into_path(), read_only))
            .collect();
        config.env = self.env;
        config.rlimits = ResourceLimits {
            memory_mb: self.memory_mb,
            cpu_time_secs: self.cpu_time_secs,
            timeout_secs: None,
        };
        config.cpu_count = self.cpu_count;
        config.dep_mounts = self
            .dep_mounts
            .into_iter()
            .map(|(source, target)| (source.into_path(), target.into_path()))
            .collect();
        config.stage_overlay =
            self.stage_overlay
                .map(|(lowerdir, upperdir, workdir)| super::StageOverlay {
                    lowerdir: lowerdir.into_path(),
                    upperdir: upperdir.into_path(),
                    workdir: workdir.into_path(),
                });
        Ok((
            self.status_path.into_path(),
            config,
            self.command,
            self.args,
        ))
    }
}

#[derive(Debug, Serialize, Deserialize)]
enum HelperState {
    Ready,
    Complete,
    Error(String),
}

pub(super) fn run_parent(
    config: &mut IsolationConfig,
    command: &str,
    args: &[String],
) -> Result<IsolationOutput> {
    config.validate()?;

    let helper_executable = match &config.helper_executable {
        Some(path) => path.clone(),
        None => std::env::current_exe()
            .map_err(|error| IsolationError::io("locate isolation helper executable", error))?,
    };
    let status_file = tempfile::NamedTempFile::new()
        .map_err(|error| IsolationError::io("create isolation helper status file", error))?;
    let request = HelperRequest::new(config, status_file.path(), command, args);
    let request = serde_json::to_vec(&request).map_err(|error| IsolationError::System {
        operation: "encode isolation helper request",
        message: error.to_string(),
    })?;
    let mut request_file = tempfile::tempfile()
        .map_err(|error| IsolationError::io("create isolation helper request file", error))?;
    request_file
        .write_all(&request)
        .map_err(|error| IsolationError::io("write isolation helper request", error))?;
    request_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| IsolationError::io("rewind isolation helper request", error))?;

    let child = std::process::Command::new(&helper_executable)
        .arg(INTERNAL_ARGUMENT)
        .stdin(Stdio::from(request_file))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| IsolationError::io("start isolation helper", error))?;

    let output = super::process::supervise(child, config, false)?;
    match read_state(status_file.path())? {
        HelperState::Complete => Ok(output),
        HelperState::Error(message) => Err(IsolationError::Setup(message)),
        HelperState::Ready if !output.status.success() => Ok(output),
        HelperState::Ready => Err(IsolationError::Setup(
            "isolation helper exited without completing its request".to_string(),
        )),
    }
}

fn write_state(path: &Path, state: &HelperState) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(state).map_err(std::io::Error::other)?;
    std::fs::write(path, bytes)
}

fn read_state(path: &Path) -> Result<HelperState> {
    let bytes = std::fs::read(path)
        .map_err(|error| IsolationError::io("read isolation helper status", error))?;
    if bytes.is_empty() {
        return Err(IsolationError::Setup(
            "isolation helper did not acknowledge the protocol request".to_string(),
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        IsolationError::Setup(format!(
            "isolation helper returned an invalid protocol status: {error}"
        ))
    })
}

pub(super) fn is_helper_process() -> bool {
    std::env::args_os().nth(1).as_deref() == Some(OsStr::new(INTERNAL_ARGUMENT))
}

pub(super) fn run_helper_process() -> ! {
    let request = match serde_json::from_reader::<_, HelperRequest>(std::io::stdin().lock()) {
        Ok(request) => request,
        Err(error) => {
            crate::errln!("invalid isolation helper request: {error}");
            exit(125);
        }
    };
    let status_path = request.status_path.clone().into_path();
    let (status_path, config, command, args) = match request.into_parts() {
        Ok(parts) => parts,
        Err(error) => {
            let _ = write_state(&status_path, &HelperState::Error(error.to_string()));
            exit(125);
        }
    };
    if let Err(error) = write_state(&status_path, &HelperState::Ready) {
        crate::errln!("write isolation helper ready state: {error}");
        exit(125);
    }

    match super::native::run_in_helper(&config, &command, &args) {
        Ok(status) => {
            if let Err(error) = write_state(&status_path, &HelperState::Complete) {
                crate::errln!("write isolation helper completion state: {error}");
                exit(125);
            }
            propagate_status(status)
        }
        Err(error) => {
            let _ = write_state(&status_path, &HelperState::Error(error.to_string()));
            exit(125);
        }
    }
}

fn propagate_status(status: std::process::ExitStatus) -> ! {
    if let Some(code) = status.code() {
        exit(code);
    }
    if let Some(signal) = status.signal() {
        // Restore default handling so the application process observes the
        // same signal status it would have received from the stage itself.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
            libc::_exit(128 + signal);
        }
    }
    exit(125)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_paths_roundtrip_non_utf8_bytes() {
        let original = PathBuf::from(OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff]));
        let encoded = serde_json::to_vec(&WirePath::from_path(&original)).unwrap();
        let decoded: WirePath = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.into_path(), original);
    }

    #[test]
    fn request_roundtrips_isolation_configuration() {
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let status = tempfile::NamedTempFile::new().unwrap();
        let mut config = IsolationConfig::new(
            IsolationLevel::Relaxed,
            root.path().to_path_buf(),
            output.path().to_path_buf(),
            "helper-roundtrip".to_string(),
        );
        config.env.push(("KEY".to_string(), "value".to_string()));
        config.rlimits.timeout_secs = Some(9);
        let request =
            HelperRequest::new(&config, status.path(), "/bin/true", &["--flag".to_string()]);
        let bytes = serde_json::to_vec(&request).unwrap();
        let decoded: HelperRequest = serde_json::from_slice(&bytes).unwrap();
        let (_, decoded, command, args) = decoded.into_parts().unwrap();

        assert_eq!(decoded.level, IsolationLevel::Relaxed);
        assert_eq!(decoded.src_dir, config.src_dir);
        assert_eq!(decoded.output_dir, config.output_dir);
        assert_eq!(decoded.env, config.env);
        // Timeout stays on the application side of the protocol boundary.
        assert_eq!(decoded.rlimits.timeout_secs, None);
        assert_eq!(command, "/bin/true");
        assert_eq!(args, vec!["--flag"]);
    }

    #[test]
    fn request_roundtrips_stage_overlay() {
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let status = tempfile::NamedTempFile::new().unwrap();
        let mut config = IsolationConfig::new(
            IsolationLevel::Strict,
            root.path().to_path_buf(),
            output.path().to_path_buf(),
            "helper-overlay".to_string(),
        );
        config.stage_overlay = Some(crate::isolation::StageOverlay {
            lowerdir: PathBuf::from("/var/tmp/wright/workshop/pkg-1.0/base"),
            upperdir: PathBuf::from("/var/tmp/wright/workshop/pkg-1.0/layers/03-compile"),
            workdir: PathBuf::from("/var/tmp/wright/workshop/pkg-1.0/.ovl_work/03-compile"),
        });
        let request = HelperRequest::new(&config, status.path(), "/bin/true", &["-x".to_string()]);
        let bytes = serde_json::to_vec(&request).unwrap();
        let decoded: HelperRequest = serde_json::from_slice(&bytes).unwrap();
        let (_, decoded, _, _) = decoded.into_parts().unwrap();

        let overlay = decoded
            .stage_overlay
            .expect("overlay must survive the wire");
        assert_eq!(
            overlay.lowerdir,
            PathBuf::from("/var/tmp/wright/workshop/pkg-1.0/base")
        );
        assert_eq!(
            overlay.upperdir,
            PathBuf::from("/var/tmp/wright/workshop/pkg-1.0/layers/03-compile")
        );
        assert_eq!(
            overlay.workdir,
            PathBuf::from("/var/tmp/wright/workshop/pkg-1.0/.ovl_work/03-compile")
        );
    }

    #[test]
    fn incompatible_helper_fails_the_protocol_closed() {
        let helper = Path::new("/bin/true");
        if !helper.is_file() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut config = IsolationConfig::new(
            IsolationLevel::Strict,
            root.path().to_path_buf(),
            output.path().to_path_buf(),
            "bad-helper".to_string(),
        );
        config.helper_executable = Some(helper.to_path_buf());

        let error = match run_parent(&mut config, "/bin/true", &[]) {
            Ok(_) => panic!("an incompatible helper must not be accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("did not acknowledge"));
    }
}
