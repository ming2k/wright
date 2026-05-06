use serde::Serialize;

use super::errors::Result;
use super::id::{canonical_json, WorkflowId};
use super::step::{ScheduledStep, Step};

/// A complete, ready-to-execute workflow.
///
/// Produced by `WorkflowBuilder::build`. The `workflow_id` is content-derived
/// from `kind` + canonical `inputs_json`; the `steps` are ordered by insertion
/// for determinism but the runner schedules by the dependency graph, not order.
pub struct WorkflowSpec {
    pub kind: &'static str,
    pub workflow_id: WorkflowId,
    pub inputs_json: String,
    pub steps: Vec<ScheduledStep>,
}

/// Builder for a `WorkflowSpec`.
///
/// The builder owns the workflow id, so each `add()` can mint a content-derived
/// `StepId` that callers can use as a dependency reference for downstream steps.
pub struct WorkflowBuilder {
    workflow_id: WorkflowId,
    kind: &'static str,
    inputs_json: String,
    steps: Vec<ScheduledStep>,
}

impl WorkflowBuilder {
    /// Start a new workflow.
    ///
    /// `inputs` should be the canonical, normalized form of the user-facing
    /// arguments (sorted, deduped, lowercased where relevant). Two invocations
    /// with semantically identical `inputs` MUST hash to the same workflow id;
    /// otherwise resume will silently start a new run instead of continuing.
    pub fn new<I: Serialize>(kind: &'static str, inputs: &I) -> Result<Self> {
        let inputs_json = canonical_json(inputs)?;
        let workflow_id = WorkflowId::derive(kind, &inputs_json);
        Ok(Self {
            workflow_id,
            kind,
            inputs_json,
            steps: Vec::new(),
        })
    }

    pub fn workflow_id(&self) -> &WorkflowId {
        &self.workflow_id
    }

    /// Schedule a step. Returns its `StepId` so downstream steps can list it
    /// in their `depends_on`.
    pub fn add<S: Step>(&mut self, step: S) -> Result<super::id::StepId> {
        let scheduled = ScheduledStep::from_step(&self.workflow_id, step)?;
        let id = scheduled.id.clone();
        self.steps.push(scheduled);
        Ok(id)
    }

    pub fn build(self) -> WorkflowSpec {
        WorkflowSpec {
            kind: self.kind,
            workflow_id: self.workflow_id,
            inputs_json: self.inputs_json,
            steps: self.steps,
        }
    }
}
