use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("Cache IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Cache error: {0}")]
    Message(String),
}

pub type Result<T, E = CacheError> = std::result::Result<T, E>;
