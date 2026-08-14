#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("parse error: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("validation error: {0}")]
    ValidationError(String),

    #[error("TOML deserialization error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error(transparent)]
    Model(#[from] wright_model::ModelError),

    /// Structured context wrapper: keeps the underlying error as a real
    /// `source()` so failure reports walk the actual chain instead of
    /// re-parsing flattened text. Display still nests (`"{msg}: {source}"`),
    /// so single-line logs keep the full chain. Construct via
    /// [`PlanError::context`].
    #[error("{msg}: {source}")]
    Context {
        msg: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl PlanError {
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

pub type Result<T> = std::result::Result<T, PlanError>;

pub(crate) use PlanError as WrightError;
