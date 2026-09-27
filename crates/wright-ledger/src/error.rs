use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("Ledger IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Ledger format error: {0}")]
    Format(String),
}

pub type Result<T, E = LedgerError> = std::result::Result<T, E>;
