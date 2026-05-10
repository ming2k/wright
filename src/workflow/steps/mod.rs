//! Concrete step implementations for Wright's workflows.
//!
//! Each step wraps an existing business operation:
//! * `BuildPlanStep`     — build the lifecycle pipeline of one plan
//! * `PackagePlanStep`   — slice + archive one plan's outputs
//! * `InstallBatchStep`  — install a coherent batch of parts (root mutator)
//! * `ApplyConfigStep`   — apply a group's `[config]` block (hostname, tz, etc.)
//!
//! Steps own their internal idempotence: re-running a step with the same
//! inputs whose artifacts already exist on disk should be cheap and produce
//! the same outputs.

mod apply_config;
mod build_plan;
mod install_batch;
mod package_plan;

pub use apply_config::{ApplyConfigInputs, ApplyConfigStep};
pub use build_plan::{
    BuildOptionsCanonical, BuildPlanInputs, BuildPlanOutputs, BuildPlanStep, PlanRef,
};
pub use install_batch::{
    ArchiveRef, InstallBatchInputs, InstallBatchOutputs, InstallBatchStep, InstallSource,
};
pub use package_plan::{PackagePlanInputs, PackagePlanOutputs, PackagePlanStep};
