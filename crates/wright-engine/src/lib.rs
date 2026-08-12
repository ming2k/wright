//! Wright application engine.
//!
//! This crate orchestrates plan resolution, builds, sealing, deployment, and
//! system queries. It contains no clap argument definitions or binary entry
//! point.

mod cancellation;
pub mod config;
pub mod error;
pub mod foundry;
pub mod identify;
pub mod isolation;
pub mod operations;
pub mod query;
pub mod resolve;
pub mod seal;
pub mod transaction;
pub mod util;
