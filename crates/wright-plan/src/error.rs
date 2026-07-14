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
}

pub type Result<T> = std::result::Result<T, PlanError>;

pub(crate) use PlanError as WrightError;
