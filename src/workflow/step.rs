use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::watch;

use super::errors::{Result as WfResult, WorkflowError};
use super::id::{RunId, StepId, WorkflowId};
use crate::database::InstalledDb;

/// Coarse-grained scheduling resource. The runner enforces per-class concurrency
/// (semaphores for `Cpu`/`Network`, a global mutex for `RootMutator`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceClass {
    /// CPU-bound work; bounded by the CPU pool.
    Cpu,
    /// Mutates the target root or DB; serialized globally.
    RootMutator,
    /// Network-bound work (fetch); bounded by a separate pool.
    Network,
    /// Cheap, unbounded steps (planning, dry-run, etc.).
    Trivial,
}

/// Step lifecycle state, persisted in the `workflow_steps` table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Succeeded => "succeeded",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Status::Pending),
            "running" => Some(Status::Running),
            "succeeded" => Some(Status::Succeeded),
            "failed" => Some(Status::Failed),
            "skipped" => Some(Status::Skipped),
            _ => None,
        }
    }
}

/// Final state of a run, persisted in the `workflow_runs` table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Succeeded,
    Failed,
    Aborted,
}

impl TerminalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalStatus::Succeeded => "succeeded",
            TerminalStatus::Failed => "failed",
            TerminalStatus::Aborted => "aborted",
        }
    }
}

/// Per-execution context handed to a step's `execute()`.
///
/// Carries everything the step needs to run: db handle, log destination,
/// cancellation signal, and the JSON outputs of upstream steps.
pub struct StepContext {
    pub run_id: RunId,
    pub workflow_id: WorkflowId,
    pub step_id: StepId,
    pub db: Arc<InstalledDb>,
    pub log_dir: PathBuf,
    pub cancel: watch::Receiver<bool>,
    /// JSON-encoded outputs of upstream steps, keyed by their `StepId`.
    /// Each value is the serialized form of that step's `Outputs` type.
    pub upstream_outputs: HashMap<StepId, serde_json::Value>,
}

/// A unit of resumable work.
///
/// Contract:
/// * `execute` MUST be idempotent given the same `Inputs`. Re-running a step
///   whose outputs already exist on disk should be cheap and produce the same
///   `Outputs`.
/// * `Inputs` MUST be serializable to canonical JSON in a way that is stable
///   across processes (no `HashMap`, no time-dependent fields). Use
///   `BTreeMap` and pre-sorted `Vec` for set-like inputs.
/// * `Outputs` should reference artifacts (paths, hashes) rather than
///   embedding bulk data; the runner persists them in `workflow_steps.outputs_json`.
pub trait Step: Send + Sync + 'static {
    type Inputs: Serialize + DeserializeOwned + Send + Sync;
    type Outputs: Serialize + DeserializeOwned + Send + Sync;

    /// Stable identifier for this step kind. Hashed into the `StepId`.
    const KIND: &'static str;

    /// Scheduling class.
    const RESOURCE_CLASS: ResourceClass;

    fn inputs(&self) -> &Self::Inputs;

    fn depends_on(&self) -> &[StepId] {
        &[]
    }

    fn execute(self: Arc<Self>, ctx: StepContext) -> BoxFuture<'static, WfResult<Self::Outputs>>;
}

/// Type-erased shape of a `Step` ready for the runner.
///
/// Constructed only via `WorkflowBuilder::add`. The runner treats steps as
/// opaque ids + dep edges + a JSON-returning closure.
pub struct ScheduledStep {
    pub id: StepId,
    pub kind: &'static str,
    pub inputs_json: String,
    pub depends_on: Vec<StepId>,
    pub class: ResourceClass,
    pub run:
        Box<dyn Fn(StepContext) -> BoxFuture<'static, WfResult<serde_json::Value>> + Send + Sync>,
}

impl ScheduledStep {
    pub(super) fn from_step<S: Step>(workflow_id: &WorkflowId, step: S) -> WfResult<Self> {
        let inputs_json = super::id::canonical_json(step.inputs())?;
        let id = StepId::derive(workflow_id, S::KIND, &inputs_json);
        let depends_on = step.depends_on().to_vec();
        let class = S::RESOURCE_CLASS;
        let arc = Arc::new(step);
        let run: Box<
            dyn Fn(StepContext) -> BoxFuture<'static, WfResult<serde_json::Value>> + Send + Sync,
        > = Box::new(move |ctx: StepContext| {
            let arc = arc.clone();
            Box::pin(async move {
                let out = arc.execute(ctx).await?;
                serde_json::to_value(&out).map_err(WorkflowError::Serialize)
            })
        });
        Ok(ScheduledStep {
            id,
            kind: S::KIND,
            inputs_json,
            depends_on,
            class,
            run,
        })
    }
}
