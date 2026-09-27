//! System installed package index and delivery transaction state for Wright (ADR-0043, ADR-0048).

pub mod database;
pub mod delivery;
pub mod error;

pub use database::{InstalledDb, ReadOnlyDb, RegistryQuery};
pub use error::{RegistryError, Result, StateError};
