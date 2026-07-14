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
}

pub type Result<T> = std::result::Result<T, PartError>;

pub(crate) use PartError as WrightError;
