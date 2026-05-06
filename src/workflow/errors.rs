use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("step failed: {0}")]
    StepFailed(String),

    #[error("workflow deadlock: unmet dependencies remain: {0}")]
    Deadlock(String),

    #[error("step result channel closed unexpectedly")]
    ChannelClosed,

    #[error("workflow cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl WorkflowError {
    pub fn other(msg: impl Into<String>) -> Self {
        WorkflowError::Other(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, WorkflowError>;
