//! Concrete step implementations for Wright's workflows.
//!
//! Each step wraps an existing business operation:
//! * `BuildPlanStep`     — build the lifecycle pipeline of one plan
//! * `PackagePlanStep`   — slice + archive one plan's outputs
//! * `InstallBatchStep`  — install a coherent batch of parts (root mutator)
//! * `ExtractPackStep`   — unpack a `.wright.pack.tar` into a stable staging dir
//! * `ApplyOverlayStep`  — apply a pack's `overlay.tar` to the target root
//! * `ApplyConfigStep`   — apply a pack's `[config]` block (hostname, tz, etc.)
//!
//! Steps own their internal idempotence: re-running a step with the same
//! inputs whose artifacts already exist on disk should be cheap and produce
//! the same outputs.

mod apply_config;
mod apply_overlay;
mod build_plan;
mod extract_pack;
mod install_batch;
mod package_plan;

pub use apply_config::{ApplyConfigInputs, ApplyConfigStep};
pub use apply_overlay::{ApplyOverlayInputs, ApplyOverlayOutputs, ApplyOverlayStep};
pub use build_plan::{
    BuildOptionsCanonical, BuildPlanInputs, BuildPlanOutputs, BuildPlanStep, PlanRef,
};
pub use extract_pack::{ExtractPackInputs, ExtractPackOutputs, ExtractPackStep};
pub use install_batch::{
    ArchiveRef, InstallBatchInputs, InstallBatchOutputs, InstallBatchStep, InstallSource,
};
pub use package_plan::{PackagePlanInputs, PackagePlanOutputs, PackagePlanStep};
