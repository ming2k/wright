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

    #[error("SQLite error: {0}")]
    SqliteError(#[from] sqlx::Error),
}

impl From<wright_model::ModelError> for WrightError {
    fn from(error: wright_model::ModelError) -> Self {
        match error {
            wright_model::ModelError::VersionError(message) => Self::VersionError(message),
            wright_model::ModelError::ValidationError(message) => Self::ValidationError(message),
            wright_model::ModelError::IsolationError(message) => Self::IsolationError(message),
        }
    }
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

impl From<wright_plan::PlanError> for WrightError {
    fn from(error: wright_plan::PlanError) -> Self {
        match error {
            wright_plan::PlanError::ParseError(message) => Self::ParseError(message),
            wright_plan::PlanError::IoError(error) => Self::IoError(error),
            wright_plan::PlanError::ValidationError(message) => Self::ValidationError(message),
            wright_plan::PlanError::TomlError(error) => Self::TomlError(error),
            wright_plan::PlanError::Model(error) => error.into(),
        }
    }
}

impl From<wright_part::PartError> for WrightError {
    fn from(error: wright_part::PartError) -> Self {
        match error {
            wright_part::PartError::IoError(error) => Self::IoError(error),
            wright_part::PartError::ForgeError(message) => Self::ForgeError(message),
            wright_part::PartError::PartError(message) => Self::PartError(message),
            wright_part::PartError::ValidationError(message) => Self::ValidationError(message),
        }
    }
}

impl From<wright_state::StateError> for WrightError {
    fn from(error: wright_state::StateError) -> Self {
        match error {
            wright_state::StateError::IoError(error) => Self::IoError(error),
            wright_state::StateError::DatabaseError(message) => Self::DatabaseError(message),
            wright_state::StateError::ForgeError(message) => Self::ForgeError(message),
            wright_state::StateError::DeployError(message) => Self::DeployError(message),
            wright_state::StateError::AccessDenied(message) => Self::AccessDenied(message),
            wright_state::StateError::LockError(message) => Self::LockError(message),
            wright_state::StateError::PartNotFound(message) => Self::PartNotFound(message),
            wright_state::StateError::PartAlreadyInstalled(message) => {
                Self::PartAlreadyInstalled(message)
            }
            wright_state::StateError::SqliteError(error) => Self::SqliteError(error),
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
