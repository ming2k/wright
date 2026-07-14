use std::path::PathBuf;

use wright_model::isolation::IsolationLevel;

/// Errors produced by the process-isolation boundary.
///
/// Keeping this error independent from [`crate::error::WrightError`] prevents
/// the low-level Linux runner from depending on foundry or operation errors.
#[derive(Debug, thiserror::Error)]
pub enum IsolationError {
    #[error("cancelled by user")]
    Cancelled,

    #[error("invalid isolation configuration: {0}")]
    InvalidConfig(String),

    #[error(
        "Namespace isolation unavailable for {level} mode; refusing to execute directly on the host (set isolation = \"none\" explicitly to allow host execution)"
    )]
    Unavailable { level: IsolationLevel },

    #[error("isolation level none cannot run against base root {0}")]
    UnisolatedBaseRoot(PathBuf),

    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("{operation}: {message}")]
    System {
        operation: &'static str,
        message: String,
    },

    #[error("isolation setup failed: {0}")]
    Setup(String),
}

impl IsolationError {
    pub(crate) fn io(operation: &'static str, source: std::io::Error) -> Self {
        Self::Io { operation, source }
    }

    pub(crate) fn system(operation: &'static str, source: impl std::fmt::Display) -> Self {
        Self::System {
            operation,
            message: source.to_string(),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, IsolationError>;
