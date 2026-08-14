//! Part archive formats, local stores, folios, and package validation.

pub mod archive;
pub mod compression;
pub mod elf;
pub mod error;
pub mod fhs;
pub mod folio;
pub mod platform;
pub mod soname;
pub mod store;

pub mod version {
    pub use wright_model::version::*;
}

pub use archive::*;
pub use error::{PartError, Result};
pub use version::{Version, VersionConstraint, VersionOp};
