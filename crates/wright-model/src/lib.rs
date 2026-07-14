//! Stable domain primitives shared by Wright's internal crates.
//!
//! This crate deliberately contains no filesystem, database, network, CLI, or
//! process-execution code. Keeping the dependency-free model small prevents it
//! from becoming a generic `common` crate.

pub mod isolation;
pub mod pipeline;
pub mod version;

use std::fmt;

/// Errors produced while parsing or validating domain primitives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    VersionError(String),
    ValidationError(String),
    IsolationError(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionError(message) => write!(f, "version error: {message}"),
            Self::ValidationError(message) => write!(f, "validation error: {message}"),
            Self::IsolationError(message) => write!(f, "isolation error: {message}"),
        }
    }
}

impl std::error::Error for ModelError {}

pub type Result<T> = std::result::Result<T, ModelError>;
