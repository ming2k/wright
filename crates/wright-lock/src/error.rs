use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LockError {
    #[error("failed to create lock directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to open lock file {path}: {source}")]
    OpenFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("another wright process is already running (lock held at {path})")]
    Timeout { path: PathBuf, timeout_secs: u64 },

    #[error("lock IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T, E = LockError> = std::result::Result<T, E>;
