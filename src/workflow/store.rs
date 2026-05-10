use std::collections::{HashMap, HashSet};

use serde_json::Value as JsonValue;

use super::errors::Result;
use super::id::{StepId, WorkflowId};
use super::step::{ScheduledStep, Status};
use crate::database::InstalledDb;

/// Summary of a workflow row, for diagnostic commands like `wright ps`.
#[derive(Debug)]
pub struct WorkflowSummary {
    pub id: WorkflowId,
    pub kind: String,
}

/// Summary of a workflow step row, for diagnostic commands.
#[derive(Debug)]
pub struct StepSummary {
    pub id: StepId,
    pub status: Status,
    pub attempt: i32,
    pub plan_name: Option<String>,
    pub label: Option<String>,
}

/// Sole owner of the `workflows` and `workflow_steps` tables. No other module
/// talks to them directly.
pub struct WorkflowStore<'a> {
    db: &'a InstalledDb,
}

impl<'a> WorkflowStore<'a> {
    pub fn new(db: &'a InstalledDb) -> Self {
        Self { db }
    }

    /// List all workflows in the database.
    pub async fn list_workflows(&self) -> Result<Vec<WorkflowSummary>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, kind FROM workflows ORDER BY created_at DESC")
                .fetch_all(&self.db.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(id, kind)| WorkflowSummary {
                id: WorkflowId::from(id),
                kind,
            })
            .collect())
    }

    /// List all steps for a workflow, including basic metadata for display.
    pub async fn list_steps(&self, workflow_id: &WorkflowId) -> Result<Vec<StepSummary>> {
        let rows: Vec<(String, String, i32, String, String)> = sqlx::query_as(
            "SELECT id, status, attempt, inputs_json, kind \
             FROM workflow_steps \
             WHERE workflow_id = ? \
             ORDER BY id",
        )
        .bind(workflow_id.as_str())
        .fetch_all(&self.db.pool)
        .await?;

        let mut out = Vec::new();
        for (id, status_str, attempt, inputs_json, kind) in rows {
            let status = Status::parse(&status_str).unwrap_or(Status::Pending);
            let inputs: JsonValue = serde_json::from_str(&inputs_json).unwrap_or(JsonValue::Null);
            let plan_name = extract_plan_name(&inputs, &kind);
            let label = inputs
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            out.push(StepSummary {
                id: StepId::from(id),
                status,
                attempt,
                plan_name,
                label,
            });
        }
        Ok(out)
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    pub async fn upsert_workflow(
        &self,
        id: &WorkflowId,
        kind: &str,
        inputs_json: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO workflows (id, kind, inputs_json, created_at) \
             VALUES (?, ?, ?, ?) \
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(id.as_str())
        .bind(kind)
        .bind(inputs_json)
        .bind(Self::now_ms())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Insert the step row if absent. Existing rows are left untouched —
    /// that's what makes resume preserve `succeeded` status across attempts.
    pub async fn upsert_step(&self, workflow_id: &WorkflowId, s: &ScheduledStep) -> Result<()> {
        let depends_on_json =
            serde_json::to_string(&s.depends_on.iter().map(|d| d.as_str()).collect::<Vec<_>>())?;
        sqlx::query(
            "INSERT INTO workflow_steps \
             (id, workflow_id, kind, inputs_json, depends_on_json, status, attempt) \
             VALUES (?, ?, ?, ?, ?, 'pending', 0) \
             ON CONFLICT(id) DO NOTHING",
        )
        .bind(s.id.as_str())
        .bind(workflow_id.as_str())
        .bind(s.kind)
        .bind(&s.inputs_json)
        .bind(&depends_on_json)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Remove persisted rows that do not belong to the current in-memory spec.
    ///
    /// Workflow identity is intentionally based on command intent, so commands
    /// like `apply` can produce a different step graph as installed state
    /// changes. Rows from an older graph are stale resume state and must not
    /// participate in scheduling or diagnostics for the current graph.
    pub async fn prune_steps_not_in(
        &self,
        workflow_id: &WorkflowId,
        current_steps: &[ScheduledStep],
    ) -> Result<()> {
        let current: HashSet<&str> = current_steps.iter().map(|s| s.id.as_str()).collect();
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT id FROM workflow_steps WHERE workflow_id = ?")
                .bind(workflow_id.as_str())
                .fetch_all(&self.db.pool)
                .await?;

        for id in rows {
            if !current.contains(id.as_str()) {
                sqlx::query("DELETE FROM workflow_steps WHERE workflow_id = ? AND id = ?")
                    .bind(workflow_id.as_str())
                    .bind(id)
                    .execute(&self.db.pool)
                    .await?;
            }
        }

        Ok(())
    }

    pub async fn load_attempts(&self, workflow_id: &WorkflowId) -> Result<HashMap<StepId, i32>> {
        let rows: Vec<(String, i32)> =
            sqlx::query_as("SELECT id, attempt FROM workflow_steps WHERE workflow_id = ?")
                .bind(workflow_id.as_str())
                .fetch_all(&self.db.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(id, attempt)| (StepId::from(id), attempt))
            .collect())
    }

    pub async fn load_statuses(&self, workflow_id: &WorkflowId) -> Result<HashMap<StepId, Status>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, status FROM workflow_steps WHERE workflow_id = ?")
                .bind(workflow_id.as_str())
                .fetch_all(&self.db.pool)
                .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, st)| Status::parse(&st).map(|s| (StepId::from(id), s)))
            .collect())
    }

    pub async fn load_outputs(
        &self,
        workflow_id: &WorkflowId,
    ) -> Result<HashMap<StepId, JsonValue>> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, outputs_json FROM workflow_steps \
             WHERE workflow_id = ? AND status = 'succeeded'",
        )
        .bind(workflow_id.as_str())
        .fetch_all(&self.db.pool)
        .await?;
        let mut out = HashMap::new();
        for (id, json) in rows {
            if let Some(json) = json {
                let v: JsonValue = serde_json::from_str(&json)?;
                out.insert(StepId::from(id), v);
            }
        }
        Ok(out)
    }

    /// Reset any orphaned `running` rows in this workflow back to `pending`.
    /// The database lock guarantees no other process owns them.
    pub async fn reset_running(&self, workflow_id: &WorkflowId) -> Result<()> {
        sqlx::query(
            "UPDATE workflow_steps SET status = 'pending', started_at = NULL \
             WHERE workflow_id = ? AND status = 'running'",
        )
        .bind(workflow_id.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_running(&self, step: &StepId) -> Result<()> {
        sqlx::query(
            "UPDATE workflow_steps \
             SET status = 'running', started_at = ?, attempt = attempt + 1, \
                 finished_at = NULL, outputs_json = NULL, failure_json = NULL \
             WHERE id = ?",
        )
        .bind(Self::now_ms())
        .bind(step.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_succeeded(&self, step: &StepId, outputs: &JsonValue) -> Result<()> {
        let outputs_json = serde_json::to_string(outputs)?;
        sqlx::query(
            "UPDATE workflow_steps \
             SET status = 'succeeded', finished_at = ?, outputs_json = ?, failure_json = NULL \
             WHERE id = ?",
        )
        .bind(Self::now_ms())
        .bind(outputs_json)
        .bind(step.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_failed(&self, step: &StepId, failure: &JsonValue) -> Result<()> {
        let failure_json = serde_json::to_string(failure)?;
        sqlx::query(
            "UPDATE workflow_steps \
             SET status = 'failed', finished_at = ?, failure_json = ? \
             WHERE id = ?",
        )
        .bind(Self::now_ms())
        .bind(failure_json)
        .bind(step.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn load_failures(
        &self,
        workflow_id: &WorkflowId,
    ) -> Result<HashMap<StepId, JsonValue>> {
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, failure_json FROM workflow_steps \
             WHERE workflow_id = ? AND status = 'failed'",
        )
        .bind(workflow_id.as_str())
        .fetch_all(&self.db.pool)
        .await?;
        let mut out = HashMap::new();
        for (id, json) in rows {
            if let Some(json) = json {
                let v: JsonValue = serde_json::from_str(&json)?;
                out.insert(StepId::from(id), v);
            }
        }
        Ok(out)
    }

    pub async fn clear_workflow(&self, workflow_id: &WorkflowId) -> Result<()> {
        sqlx::query("DELETE FROM workflows WHERE id = ?")
            .bind(workflow_id.as_str())
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}

/// Extract a human-readable plan name from step inputs_json, if present.
fn extract_plan_name(inputs: &JsonValue, kind: &str) -> Option<String> {
    // build_plan and package_plan steps embed the plan name under inputs.plan.name
    if kind == "build_plan" || kind == "package_plan" {
        inputs
            .get("plan")
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}
