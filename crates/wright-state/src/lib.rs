//! Persistent installed state, delivery recovery, CAS, and process locking.

pub mod cas;
pub mod database;
pub mod delivery;
pub mod error;
pub mod ledger;
pub mod lock;

pub use database::InstalledDb;
pub use error::{Result, StateError};
