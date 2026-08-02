#[derive(Debug, thiserror::Error)]
pub enum WrightError {
    #[error("parse error: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("forge error: {0}")]
    ForgeError(String),

    #[error("deploy error: {0}")]
    DeployError(String),

    #[error("remove error: {0}")]
    RemoveError(String),

    #[error("part error: {0}")]
    PartError(String),

    #[error("config error: {0}")]
    ConfigError(String),

    #[error("access denied: {0}. (hint: try running with sudo)")]
    AccessDenied(String),

    #[error("lock error: {0}")]
    LockError(String),

    #[error("version error: {0}")]
    VersionError(String),

    #[error("dependency error: {0}")]
    DependencyError(String),

    #[error("part not found: {0}")]
    PartNotFound(String),

    #[error("part already deployed: {0}")]
    PartAlreadyInstalled(String),

    #[error("upgrade error: {0}")]
    UpgradeError(String),

    #[error("script error: {0}")]
    ScriptError(String),

    #[error("validation error: {0}")]
    ValidationError(String),

    #[error("isolation error: {0}")]
    IsolationError(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("TOML deserialization error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error(transparent)]
    Model(#[from] wright_model::ModelError),

    #[error(transparent)]
    Plan(#[from] wright_plan::PlanError),

    #[error(transparent)]
    Part(#[from] wright_part::PartError),

    #[error(transparent)]
    State(#[from] wright_state::StateError),
}

impl From<crate::isolation::IsolationError> for WrightError {
    fn from(error: crate::isolation::IsolationError) -> Self {
        match error {
            crate::isolation::IsolationError::Cancelled => {
                Self::ForgeError("cancelled by user".into())
            }
            error => Self::IsolationError(error.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, WrightError>;

/// Extension trait that adds `.context()` to any Result,
/// converting errors into WrightError::ForgeError with a context message.
pub trait WrightResultExt<T> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T>;
}

impl<T, E: std::fmt::Display> WrightResultExt<T> for std::result::Result<T, E> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T> {
        self.map_err(|e| WrightError::ForgeError(format!("{}: {}", msg, e)))
    }
}
