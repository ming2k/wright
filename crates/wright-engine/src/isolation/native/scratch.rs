use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tracing::debug;

use crate::isolation::IsolationConfig;
use crate::isolation::error::{IsolationError, Result};

static NEXT_SCRATCH_ID: AtomicU64 = AtomicU64::new(1);

/// Derive the scratch directory for isolation setup from the active build root.
///
/// `src_dir` is `<build_root>/src` for normal builds, so placing scratch
/// directories under its parent keeps temporary overlay state on the same
/// filesystem as the rest of the build instead of hardcoding `/tmp`.
pub(super) fn isolation_scratch_base(config: &IsolationConfig) -> PathBuf {
    let build_root = config.src_dir.parent().unwrap_or(config.src_dir.as_path());
    let run_id = NEXT_SCRATCH_ID.fetch_add(1, Ordering::Relaxed);
    build_root.join(".wright-isolation").join(format!(
        "{}-{}-{run_id}",
        config.task_id,
        std::process::id()
    ))
}

/// Remove the temporary overlay and isolation-root directories for a given task.
///
/// These directories are created inside the forked child's mount namespace.
/// The mounts are automatically cleaned up when the namespace is destroyed,
/// but the empty directory trees can persist on the host filesystem after
/// crashes or forced termination.
pub(super) fn cleanup_isolation_dirs(scratch: &Path) {
    for attempt in 0..6 {
        match std::fs::remove_dir_all(scratch) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error)
                if attempt < 5
                    && matches!(
                        error.raw_os_error(),
                        Some(libc::EBUSY) | Some(libc::ENOTEMPTY)
                    ) =>
            {
                std::thread::sleep(Duration::from_millis(10 * (1_u64 << attempt)));
            }
            Err(error) => {
                debug!(
                    event = "isolation.cleanup_failed",
                    path = %scratch.display(),
                    error = %error,
                    "Failed to clean up isolation scratch directory"
                );
                return;
            }
        }
    }
}

pub(super) fn prepare_isolation_dirs(scratch: &Path) -> Result<()> {
    let scratch_parent = scratch.parent().ok_or_else(|| {
        IsolationError::InvalidConfig(format!(
            "scratch directory has no parent: {}",
            scratch.display()
        ))
    })?;

    if let Ok(metadata) = std::fs::symlink_metadata(scratch_parent)
        && !metadata.file_type().is_dir()
    {
        return Err(IsolationError::InvalidConfig(format!(
            "isolation scratch parent is not a directory: {}",
            scratch_parent.display()
        )));
    }
    std::fs::create_dir_all(scratch_parent)
        .map_err(|error| IsolationError::io("create isolation scratch parent", error))?;
    std::fs::set_permissions(scratch_parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| IsolationError::io("secure isolation scratch parent", error))?;

    let result = (|| {
        std::fs::create_dir(scratch).map_err(|error| {
            IsolationError::io("create unique isolation scratch directory", error)
        })?;
        std::fs::set_permissions(scratch, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| IsolationError::io("secure isolation scratch directory", error))?;

        for name in ["root", "upper", "work"] {
            std::fs::create_dir(scratch.join(name))
                .map_err(|error| IsolationError::io("create isolation mount directory", error))?;
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(scratch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::isolation::IsolationLevel;

    fn config(src: &Path, output: &Path) -> IsolationConfig {
        IsolationConfig::new(
            IsolationLevel::Strict,
            src.to_path_buf(),
            output.to_path_buf(),
            "native-test".to_string(),
        )
    }

    #[test]
    fn scratch_paths_are_unique_per_run() {
        let src = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let config = config(src.path(), output.path());

        assert_ne!(
            isolation_scratch_base(&config),
            isolation_scratch_base(&config)
        );
    }
}
