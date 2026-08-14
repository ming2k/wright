#[derive(Debug, thiserror::Error)]
pub enum PartError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("forge error: {0}")]
    ForgeError(String),

    #[error("part error: {0}")]
    PartError(String),

    #[error("validation error: {0}")]
    ValidationError(String),

    /// Structured context wrapper: keeps the underlying error as a real
    /// `source()` so failure reports walk the actual chain instead of
    /// re-parsing flattened text. Display still nests (`"{msg}: {source}"`),
    /// so single-line logs keep the full chain. Construct via
    /// [`PartError::context`].
    #[error("{msg}: {source}")]
    Context {
        msg: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl PartError {
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

pub type Result<T> = std::result::Result<T, PartError>;

pub(crate) use PartError as WrightError;
