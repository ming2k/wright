use std::collections::HashMap;

use serde_json::Value as JsonValue;

use super::errors::Result;
use super::id::{RunId, StepId, WorkflowId};
use super::step::{ScheduledStep, Status, TerminalStatus};
use crate::database::InstalledDb;

/// Sole owner of the `workflows` / `workflow_steps` / `workflow_runs` /
/// `workflow_step_events` tables. No other module talks to them directly.
pub struct WorkflowStore<'a> {
    db: &'a InstalledDb,
}

impl<'a> WorkflowStore<'a> {
    pub fn new(db: &'a InstalledDb) -> Self {
        Self { db }
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
    /// that's what makes resume preserve `succeeded` status across runs.
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

    pub async fn start_run(&self, run: &RunId, workflow_id: &WorkflowId) -> Result<()> {
        let now = Self::now_ms();
        sqlx::query(
            "INSERT INTO workflow_runs (id, workflow_id, started_at, last_active_at, terminal_status) \
             VALUES (?, ?, ?, ?, NULL)",
        )
        .bind(run.as_str())
        .bind(workflow_id.as_str())
        .bind(now)
        .bind(now)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn finish_run(&self, run: &RunId, terminal: TerminalStatus) -> Result<()> {
        sqlx::query(
            "UPDATE workflow_runs SET terminal_status = ?, last_active_at = ? WHERE id = ?",
        )
        .bind(terminal.as_str())
        .bind(Self::now_ms())
        .bind(run.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn heartbeat(&self, run: &RunId) -> Result<()> {
        sqlx::query("UPDATE workflow_runs SET last_active_at = ? WHERE id = ?")
            .bind(Self::now_ms())
            .bind(run.as_str())
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_running(&self, step: &StepId) -> Result<()> {
        sqlx::query(
            "UPDATE workflow_steps \
             SET status = 'running', started_at = ?, attempt = attempt + 1, \
                 finished_at = NULL, error_text = NULL \
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
             SET status = 'succeeded', finished_at = ?, outputs_json = ?, error_text = NULL \
             WHERE id = ?",
        )
        .bind(Self::now_ms())
        .bind(outputs_json)
        .bind(step.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_failed(&self, step: &StepId, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE workflow_steps \
             SET status = 'failed', finished_at = ?, error_text = ? \
             WHERE id = ?",
        )
        .bind(Self::now_ms())
        .bind(error)
        .bind(step.as_str())
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn record_event(
        &self,
        run: &RunId,
        step: &StepId,
        event: &str,
        detail: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO workflow_step_events (run_id, step_id, event, at, detail) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(run.as_str())
        .bind(step.as_str())
        .bind(event)
        .bind(Self::now_ms())
        .bind(detail)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
}
