use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Configuration error: {0}")]
    Figment(#[from] Box<figment::Error>),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid configuration: {0}")]
    Message(String),
}

pub type Result<T, E = ConfigError> = std::result::Result<T, E>;
