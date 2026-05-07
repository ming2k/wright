//! Workflow constructors — one per top-level command.
//!
//! Each builder takes the user's CLI inputs and a `GlobalConfig`, runs any
//! up-front planning needed (resolve, dep graph, batch grouping), and
//! returns a `WorkflowSpec` ready for `drive()`.
//!
//! Workflow ids are derived from the canonicalized CLI inputs only —
//! resolved-state-dependent parts of the plan (which packages exist,
//! what's installed) appear as steps, not as inputs. This means rerunning
//! the same command after the system state has changed yields the same
//! workflow id but possibly a different set of step ids; orphaned step
//! rows from previous attempts remain harmless until the workflow succeeds and
//! the active workflow row is deleted.

mod apply;
mod build;
mod install;
mod launch;
mod package;

pub use apply::build_apply_workflow;
pub use build::build_build_workflow;
pub use install::{build_install_archives_workflow, build_install_targets_workflow};
pub use launch::{build_launch_pack_workflow, LaunchPackInputs};
pub use package::build_package_workflow;
