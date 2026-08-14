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

    /// Structured context wrapper: keeps the underlying error as a real
    /// `source()` so failure reports walk the actual chain instead of
    /// re-parsing flattened text. Display still nests (`"{msg}: {source}"`),
    /// so single-line logs keep the full chain. Construct via
    /// [`StateError::context`].
    #[error("{msg}: {source}")]
    Context {
        msg: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl StateError {
    /// Wrap `source` with a context message, preserving the error chain
    /// (unlike `Variant(format!("…: {}", e))`, which flattens it).
    pub fn context(
        msg: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Context {
            msg: msg.into(),
            source: source.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, StateError>;

pub(crate) use StateError as WrightError;
