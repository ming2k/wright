use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("Action {id} failed: {message}")]
    ActionFailed { id: String, message: String },

    #[error("ActionGraph cycle detected")]
    CycleDetected,

    #[error("Node not found in ActionGraph: {0}")]
    NodeNotFound(String),

    #[error("Action execution cancelled by user")]
    Cancelled,

    #[error("IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Scheduler error: {0}")]
    Other(String),
}

pub type Result<T, E = SchedulerError> = std::result::Result<T, E>;
