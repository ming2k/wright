//! Wright application engine.
//!
//! This crate orchestrates plan resolution, builds, sealing, deployment, and
//! system queries. It contains no clap argument definitions or binary entry
//! point.

pub use wright_sandbox as isolation;
pub use wright_sandbox::cancellation;
pub use wright_config as config;
pub use wright_resolve as resolve;
pub mod action;
pub mod error;
pub mod foundry;
pub mod graph;
pub mod identify;
pub mod ledger;
pub mod operations;
pub mod query;
pub mod seal;
pub mod transaction;
pub mod util;
