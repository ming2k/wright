use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Result, WrightError};

#[derive(Debug)]
pub struct ProcessLock {
    _file: File,
    path: PathBuf,
}

impl ProcessLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn acquire_named_lock(db_path: &Path, name: &str) -> Result<ProcessLock> {
    acquire_named_lock_with_timeout(db_path, name, Duration::from_secs(30))
}

pub fn acquire_db_lock(db_path: &Path) -> Result<ProcessLock> {
    acquire_lock_path_with_timeout(&db_lock_path(db_path), Duration::from_secs(30))
}

fn acquire_named_lock_with_timeout(
    db_path: &Path,
    name: &str,
    timeout: Duration,
) -> Result<ProcessLock> {
    acquire_lock_path_with_timeout(&lock_dir_for(db_path).join(format!("{name}.lock")), timeout)
}

fn acquire_flock(file: &File, lock_path: &Path, timeout: Duration) -> Result<()> {
    let mut delay = Duration::from_millis(50);
    let start = Instant::now();

    loop {
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            return Err(WrightError::LockError(format!(
                "another process is already holding {}; timed out after {}s",
                lock_path.display(),
                timeout.as_secs()
            )));
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_secs(1));
    }
}

pub fn lock_dir_for(db_path: &Path) -> PathBuf {
    let parent = db_path.parent().unwrap_or_else(|| Path::new("."));
    if let Some(root) = parent.parent() {
        root.join("lock")
    } else {
        parent.join("lock")
    }
}

pub fn db_lock_path(db_path: &Path) -> PathBuf {
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("database");
    lock_dir_for(db_path).join(format!("{file_name}.lock"))
}

fn acquire_lock_path_with_timeout(lock_path: &Path, timeout: Duration) -> Result<ProcessLock> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            WrightError::LockError(format!(
                "failed to create lock directory {}: {}",
                parent.display(),
                e
            ))
        })?;
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| {
            WrightError::LockError(format!(
                "failed to open lock file {}: {}",
                lock_path.display(),
                e
            ))
        })?;

    acquire_flock(&file, lock_path, timeout)?;

    let _ = file.set_len(0);
    let _ = writeln!(file, "pid={}", std::process::id());

    Ok(ProcessLock {
        _file: file,
        path: lock_path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{acquire_named_lock_with_timeout, db_lock_path, lock_dir_for};

    #[test]
    fn lock_dir_uses_wright_root_when_db_is_in_db_subdir() {
        let db_path = std::path::Path::new("/var/lib/wright/db/parts.db");
        assert_eq!(
            lock_dir_for(db_path),
            std::path::Path::new("/var/lib/wright/lock")
        );
    }

    #[test]
    fn db_lock_path_uses_db_filename_under_lock_dir() {
        let db_path = std::path::Path::new("/var/lib/wright/db/parts.db");
        assert_eq!(
            db_lock_path(db_path),
            std::path::Path::new("/var/lib/wright/lock/parts.db.lock")
        );
    }

    #[test]
    fn named_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db").join("parts.db");

        let _lock = acquire_named_lock_with_timeout(&db_path, "wbuild", Duration::from_millis(100))
            .unwrap();
        let err = acquire_named_lock_with_timeout(&db_path, "wbuild", Duration::from_millis(100))
            .unwrap_err();
        assert!(format!("{err}").contains("already holding"));
    }
}
