#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("cache error: {0}")]
    ForgeError(String),

    #[error("deploy error: {0}")]
    DeployError(String),

    #[error("access denied: {0}. (hint: try running with sudo)")]
    AccessDenied(String),

    #[error("lock error: {0}")]
    LockError(String),

    #[error("part not found: {0}")]
    PartNotFound(String),

    #[error("part already deployed: {0}")]
    PartAlreadyInstalled(String),

    #[error("SQLite error: {0}")]
    SqliteError(#[from] sqlx::Error),
}

pub type Result<T> = std::result::Result<T, StateError>;

pub(crate) use StateError as WrightError;
