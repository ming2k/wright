use std::path::Path;

use crate::database::InstalledDb;
use crate::workflow::Status;
use crate::workflow::WorkflowStore;
use anyhow::{Context, Result};

/// Display active workflows and their step statuses.
pub async fn execute_ps(db_path: &Path, all: bool, status_filter: Option<&str>) -> Result<()> {
    let db = InstalledDb::open(db_path).await.context("open database")?;

    let store = WorkflowStore::new(&db);
    let workflows = store.list_workflows().await.context("list workflows")?;

    if workflows.is_empty() {
        println!("no workflows found");
        return Ok(());
    }

    let mut active_found = false;

    for wf in &workflows {
        let steps = store
            .list_steps(&wf.id)
            .await
            .with_context(|| format!("list steps for workflow {}", wf.id.short()))?;

        let is_active = steps
            .iter()
            .any(|s| matches!(s.status, Status::Pending | Status::Running | Status::Failed));

        if !all && !is_active {
            continue;
        }

        if let Some(filter) = status_filter {
            let has_status = steps.iter().any(|s| s.status.as_str() == filter);
            if !has_status {
                continue;
            }
        }

        active_found = true;

        let total = steps.len();
        let pending = steps.iter().filter(|s| s.status == Status::Pending).count();
        let running = steps.iter().filter(|s| s.status == Status::Running).count();
        let succeeded = steps
            .iter()
            .filter(|s| s.status == Status::Succeeded)
            .count();
        let failed = steps.iter().filter(|s| s.status == Status::Failed).count();

        let summary_status = if running > 0 {
            "running"
        } else if failed > 0 && pending > 0 {
            "blocked"
        } else if failed > 0 {
            "failed"
        } else if pending > 0 {
            "pending"
        } else {
            "succeeded"
        };

        println!(
            "workflow {} ({}) — {}: {} total, {} pending, {} running, {} succeeded, {} failed",
            wf.id.short(),
            wf.kind,
            summary_status,
            total,
            pending,
            running,
            succeeded,
            failed
        );

        // Print failed steps with context
        let failed_steps: Vec<_> = steps
            .iter()
            .filter(|s| s.status == Status::Failed)
            .collect();
        if !failed_steps.is_empty() {
            println!("  failed steps:");
            for s in failed_steps {
                let ctx = match (&s.plan_name, &s.label) {
                    (Some(p), Some(l)) => format!(" [{} / {}]", p, l),
                    (Some(p), None) => format!(" [{}]", p),
                    (None, Some(l)) => format!(" [{}]", l),
                    (None, None) => String::new(),
                };
                println!("    {}{} ({} attempts)", s.id.short(), ctx, s.attempt);
            }
        }

        // Print running steps
        let running_steps: Vec<_> = steps
            .iter()
            .filter(|s| s.status == Status::Running)
            .collect();
        if !running_steps.is_empty() {
            println!("  running steps:");
            for s in running_steps {
                let ctx = match (&s.plan_name, &s.label) {
                    (Some(p), Some(l)) => format!(" [{} / {}]", p, l),
                    (Some(p), None) => format!(" [{}]", p),
                    (None, Some(l)) => format!(" [{}]", l),
                    (None, None) => String::new(),
                };
                println!("    {}{}", s.id.short(), ctx);
            }
        }
    }

    if !active_found && !all {
        println!("no active workflows (use --all to show completed)");
    }

    Ok(())
}
