//! Physical Action DAG and task execution abstractions (ADR-0048).

pub mod error;
pub mod graph;
pub mod planner;

pub use error::{Result, SchedulerError};
pub use graph::{ActionGraph, ActionId, ActionKind, ActionNode, ActionStatus, ResourceDemand};
pub use planner::{ActionPlanner, PackageGraphPlan, PlannerOptions};
