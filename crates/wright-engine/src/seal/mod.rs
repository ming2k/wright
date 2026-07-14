//! Seal — the third step of a Delivery.
//!
//! Seal validates output directories (FHS check, ELF lint), creates
//! `.wright.tar.zst` binary archives, and stores them in the CAS store for
//! future reuse.
//!
//! Seal does NOT perform output slicing — that is the exclusive responsibility
//! of the Mold subsystem (`src/foundry/mold.rs`). Seal only consumes the
//! directories that Mold produces.
//!
//! This is step 3 of the four-step Delivery flow: resolve → build → seal → deploy.

mod execute;

pub use execute::package_manifest;
