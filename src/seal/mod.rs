//! Seal — the third step of a Delivery.
//!
//! Seal validates staged build output (FHS check, ELF lint), slices output
//! directories, creates `.wright.tar.zst` binary archives, and stores them
//! in the CAS store for future reuse.
//!
//! This is step 3 of the four-step Delivery flow: resolve → forge → seal → deploy.

mod execute;

pub use execute::package_manifest;
