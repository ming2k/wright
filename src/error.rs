use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum WrightError {
    #[error("parse error: {0}")]
    ParseError(String),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("build error: {0}")]
    BuildError(String),

    #[error("install error: {0}")]
    InstallError(String),

    #[error("remove error: {0}")]
    RemoveError(String),

    #[error("archive error: {0}")]
    ArchiveError(String),

    #[error("config error: {0}")]
    ConfigError(String),

    #[error("version error: {0}")]
    VersionError(String),

    #[error("dependency error: {0}")]
    DependencyError(String),

    #[error("file conflict: {path} is already owned by package {owner}")]
    FileConflict { path: PathBuf, owner: String },

    #[error("package not found: {0}")]
    PackageNotFound(String),

    #[error("package already installed: {0}")]
    PackageAlreadyInstalled(String),

    #[error("upgrade error: {0}")]
    UpgradeError(String),

    #[error("script error: {0}")]
    ScriptError(String),

    #[error("validation error: {0}")]
    ValidationError(String),

    #[error("dockyard error: {0}")]
    DockyardError(String),

    #[error("network error: {0}")]
    NetworkError(String),

    #[error("TOML deserialization error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error("SQLite error: {0}")]
    SqliteError(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, WrightError>;

/// Extension trait that adds `.context()` to any Result,
/// converting errors into WrightError::BuildError with a context message.
/// Mirrors anyhow::Context so orchestrator/query can use familiar syntax.
pub trait WrightResultExt<T> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T>;
}

impl<T, E: std::fmt::Display> WrightResultExt<T> for std::result::Result<T, E> {
    fn context(self, msg: impl std::fmt::Display) -> Result<T> {
        self.map_err(|e| WrightError::BuildError(format!("{}: {}", msg, e)))
    }
}
