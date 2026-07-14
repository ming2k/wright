//! Plan manifest parsing, validation, and filesystem discovery.

mod checksum;
pub mod discovery;
pub mod error;
pub mod manifest;
pub mod variables;

pub use discovery::PlanIndex;
pub use error::{PlanError, Result};
pub use manifest::PlanManifest;
