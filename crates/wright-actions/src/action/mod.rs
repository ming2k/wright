//! Physical Action DAG and task execution abstractions (ADR-0048).

pub mod scheduler;

pub use scheduler::{ActionScheduler, SchedulerConfig, TaskOutcome};
pub use wright_scheduler::{
    ActionGraph, ActionId, ActionKind, ActionNode, ActionStatus, PackageGraphPlan, PlannerOptions,
    ResourceDemand, SchedulerError,
};

/// ActionPlanner lowered over BuildExecutionPlan
pub struct ActionPlanner;

impl ActionPlanner {
    pub fn plan(
        exec_plan: &crate::resolve::BuildExecutionPlan,
        opts: &PlannerOptions,
    ) -> crate::error::Result<ActionGraph> {
        wright_scheduler::ActionPlanner::plan(exec_plan, opts)
            .map_err(|e| crate::error::WrightError::ForgeError(e.to_string()))
    }
}
