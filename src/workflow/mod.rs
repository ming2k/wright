//! Workflow / Step / Run model — content-addressed, resumable pipelines.
//!
//! ## Concepts
//!
//! * **Workflow** — a DAG of steps, identified by a hash of canonical inputs.
//!   Recomputed every invocation; never read back from disk.
//! * **Step** — one resumable unit of work. Identified by a hash of
//!   `(workflow_id, kind, canonical_step_inputs)`. Idempotent.
//! * **Run** — one attempt to drive a workflow forward. Multiple runs share
//!   the workflow's steps; the steps' status rows accumulate progress across
//!   runs (this is what makes resume work without explicit flags).
//!
//! ## Design principles
//!
//! * Crash-only software (Candea & Fox): resume == startup; no special path.
//! * Content-addressing (Nix, Bazel): identity from inputs, not session ids.
//! * Tell-don't-ask: the runner flips status flags; steps are responsible
//!   for their own internal idempotence (sentinel files, hash checks, etc.).
//! * Open/closed: a new step kind is a new `impl Step`, not a schema change.
//!
//! ## Usage
//!
//! ```ignore
//! let mut b = WorkflowBuilder::new("build", &my_inputs)?;
//! let resolve_id = b.add(ResolveStep::new(targets))?;
//! let build_id = b.add(BuildPlanStep::new(plan, vec![resolve_id]))?;
//! let spec = b.build();
//! drive(db, spec, SchedulerPolicy::default(), log_dir, cancel).await?;
//! ```

pub mod builders;
mod errors;
mod id;
mod runner;
pub mod runs;
mod spec;
mod step;
pub mod steps;
mod store;

#[cfg(test)]
mod tests;

pub use errors::{Result, WorkflowError};
pub use id::{canonical_json, RunId, StepId, WorkflowId};
pub use runner::{drive, RunOutcome, SchedulerPolicy};
pub use spec::{WorkflowBuilder, WorkflowSpec};
pub use step::{ResourceClass, ScheduledStep, Status, Step, StepContext, TerminalStatus};
pub use store::WorkflowStore;
