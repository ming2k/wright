use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("Dependency cycle detected: {0}")]
    Cycle(String),

    #[error("Target not found: {0}")]
    TargetNotFound(String),

    #[error("Plan error: {0}")]
    Plan(#[from] wright_plan::PlanError),

    #[error("Part error: {0}")]
    Part(#[from] wright_part::PartError),

    #[error("Registry error: {0}")]
    Registry(#[from] wright_registry::StateError),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Resolution error: {0}")]
    Message(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Build/Forge error: {0}")]
    ForgeError(String),

    #[error("{msg}: {source}")]
    Context {
        msg: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl ResolveError {
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

pub type Result<T, E = ResolveError> = std::result::Result<T, E>;

pub trait ResultExt<T> {
    fn context(self, msg: impl Into<String>) -> Result<T>;
}

impl<T, E: std::error::Error + Send + Sync + 'static> ResultExt<T> for std::result::Result<T, E> {
    fn context(self, msg: impl Into<String>) -> Result<T> {
        self.map_err(|e| ResolveError::context(msg, e))
    }
}
