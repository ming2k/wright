//! Workflow / Step / Run model — content-addressed, resumable pipelines.
//!
//! ## Concepts
//!
//! * **Workflow** — a DAG of steps, identified by a hash of canonical inputs.
//!   Recomputed every invocation; never read back from disk.
//! * **Step** — one resumable unit of work. Identified by a hash of
//!   `(workflow_id, kind, canonical_step_inputs)`. Idempotent.
//! * **Drive attempt** — one process attempt to drive a workflow forward.
//!   Failed or aborted attempts leave active step rows for resume; a succeeded
//!   attempt clears step rows so future invocations revalidate through
//!   lower-level idempotence checks.
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
mod spec;
mod step;
pub mod steps;
mod store;

#[cfg(test)]
mod tests;

pub use errors::{Result, WorkflowError};
pub use id::{canonical_json, StepId, WorkflowId};
pub use runner::{drive, RunOutcome, SchedulerPolicy};
pub use spec::{WorkflowBuilder, WorkflowSpec};
pub use step::{ResourceClass, ScheduledStep, Status, Step, StepContext, TerminalStatus};
pub use store::{StepSummary, WorkflowStore, WorkflowSummary};
