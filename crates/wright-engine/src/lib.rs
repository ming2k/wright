//! Wright application engine.
//!
//! This crate orchestrates plan resolution, builds, sealing, deployment, and
//! system queries. It contains no clap argument definitions or binary entry
//! point.

mod cancellation;
pub mod config;
pub mod error;
pub mod foundry;
pub mod isolation;
pub mod operations;
pub mod query;
pub mod resolve;
pub mod seal;
pub mod transaction;
pub mod util;

pub use wright_part as part;
pub use wright_plan as plan;
pub use wright_state::database;

pub mod delivery {
    pub use wright_state::delivery::*;

    pub mod store {
        pub use wright_state::cas::*;
    }
}
