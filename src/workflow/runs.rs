//! Read-side queries for the `wright runs` CLI subcommand.

use std::collections::HashMap;

use serde_json::Value as JsonValue;
use sqlx::Row;

use super::errors::{Result, WorkflowError};
use super::id::{RunId, StepId, WorkflowId};
use super::step::{Status, TerminalStatus};
use crate::database::InstalledDb;

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run_id: RunId,
    pub workflow_id: WorkflowId,
    pub workflow_kind: String,
    pub started_at: i64,
    pub last_active_at: i64,
    pub terminal_status: Option<TerminalStatus>,
}

#[derive(Debug, Clone)]
pub struct StepView {
    pub id: StepId,
    pub kind: String,
    pub status: Status,
    pub attempt: i32,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error_text: Option<String>,
}

pub async fn list_runs(db: &InstalledDb, limit: i64) -> Result<Vec<RunSummary>> {
    let rows = sqlx::query(
        "SELECT r.id, r.workflow_id, w.kind, r.started_at, r.last_active_at, r.terminal_status \
         FROM workflow_runs r \
         JOIN workflows w ON w.id = r.workflow_id \
         ORDER BY r.started_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&db.pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let term: Option<String> = row.try_get("terminal_status")?;
        out.push(RunSummary {
            run_id: RunId::from_string(row.try_get::<String, _>("id")?),
            workflow_id: WorkflowId::from(row.try_get::<String, _>("workflow_id")?),
            workflow_kind: row.try_get("kind")?,
            started_at: row.try_get("started_at")?,
            last_active_at: row.try_get("last_active_at")?,
            terminal_status: term.as_deref().map(|s| match s {
                "succeeded" => TerminalStatus::Succeeded,
                "failed" => TerminalStatus::Failed,
                _ => TerminalStatus::Aborted,
            }),
        });
    }
    Ok(out)
}

pub async fn get_run(db: &InstalledDb, run: &RunId) -> Result<Option<RunSummary>> {
    let row = sqlx::query(
        "SELECT r.id, r.workflow_id, w.kind, r.started_at, r.last_active_at, r.terminal_status \
         FROM workflow_runs r \
         JOIN workflows w ON w.id = r.workflow_id \
         WHERE r.id = ?",
    )
    .bind(run.as_str())
    .fetch_optional(&db.pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let term: Option<String> = row.try_get("terminal_status")?;
    Ok(Some(RunSummary {
        run_id: RunId::from_string(row.try_get::<String, _>("id")?),
        workflow_id: WorkflowId::from(row.try_get::<String, _>("workflow_id")?),
        workflow_kind: row.try_get("kind")?,
        started_at: row.try_get("started_at")?,
        last_active_at: row.try_get("last_active_at")?,
        terminal_status: term.as_deref().map(|s| match s {
            "succeeded" => TerminalStatus::Succeeded,
            "failed" => TerminalStatus::Failed,
            _ => TerminalStatus::Aborted,
        }),
    }))
}

pub async fn list_steps(db: &InstalledDb, workflow_id: &WorkflowId) -> Result<Vec<StepView>> {
    let rows = sqlx::query(
        "SELECT id, kind, status, attempt, started_at, finished_at, error_text \
         FROM workflow_steps WHERE workflow_id = ? ORDER BY started_at NULLS LAST, id",
    )
    .bind(workflow_id.as_str())
    .fetch_all(&db.pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let st: String = row.try_get("status")?;
        out.push(StepView {
            id: StepId::from(row.try_get::<String, _>("id")?),
            kind: row.try_get("kind")?,
            status: Status::parse(&st).unwrap_or(Status::Pending),
            attempt: row.try_get("attempt")?,
            started_at: row.try_get("started_at").ok(),
            finished_at: row.try_get("finished_at").ok(),
            error_text: row.try_get("error_text").ok(),
        });
    }
    Ok(out)
}

pub async fn step_inputs(
    db: &InstalledDb,
    workflow_id: &WorkflowId,
) -> Result<HashMap<StepId, JsonValue>> {
    let rows = sqlx::query("SELECT id, inputs_json FROM workflow_steps WHERE workflow_id = ?")
        .bind(workflow_id.as_str())
        .fetch_all(&db.pool)
        .await?;
    let mut out = HashMap::new();
    for row in rows {
        let id: String = row.try_get("id")?;
        let json: String = row.try_get("inputs_json")?;
        if let Ok(v) = serde_json::from_str(&json) {
            out.insert(StepId::from(id), v);
        }
    }
    Ok(out)
}

/// Delete all rows for a workflow (cascades steps and step events).
pub async fn delete_workflow(db: &InstalledDb, id: &WorkflowId) -> Result<()> {
    sqlx::query("DELETE FROM workflow_step_events WHERE step_id IN (SELECT id FROM workflow_steps WHERE workflow_id = ?)")
        .bind(id.as_str())
        .execute(&db.pool)
        .await?;
    sqlx::query("DELETE FROM workflow_runs WHERE workflow_id = ?")
        .bind(id.as_str())
        .execute(&db.pool)
        .await?;
    sqlx::query("DELETE FROM workflows WHERE id = ?")
        .bind(id.as_str())
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Garbage-collect workflows whose runs are all terminal and older than
/// `older_than_ms` ago.  Returns the number of workflows removed.
pub async fn gc_workflows(db: &InstalledDb, older_than_ms: i64) -> Result<usize> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let cutoff = now - older_than_ms;

    let rows = sqlx::query(
        "SELECT w.id FROM workflows w \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM workflow_runs r \
             WHERE r.workflow_id = w.id AND r.terminal_status IS NULL \
         ) AND ( \
             SELECT MAX(started_at) FROM workflow_runs r WHERE r.workflow_id = w.id \
         ) < ?",
    )
    .bind(cutoff)
    .fetch_all(&db.pool)
    .await?;

    let ids: Vec<String> = rows
        .into_iter()
        .map(|r| r.try_get::<String, _>("id"))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| WorkflowError::Db(e))?;

    for id in &ids {
        delete_workflow(db, &WorkflowId::from(id.clone())).await?;
    }
    Ok(ids.len())
}
