use anyhow::{Context, Result};
use std::path::Path;

use crate::cli::runs::{RunsArgs, RunsCommand};
use crate::database::InstalledDb;
use crate::workflow::runs::{gc_workflows, list_runs, list_steps};
use crate::workflow::RunId;
use crate::workflow::{Status, TerminalStatus};

pub async fn execute_runs(args: RunsArgs, db_path: &Path) -> Result<()> {
    let cmd = args.command.unwrap_or(RunsCommand::List { limit: 20 });
    match cmd {
        RunsCommand::List { limit } => list(db_path, limit).await,
        RunsCommand::Show { run_id } => show(db_path, &run_id).await,
        RunsCommand::Gc { days } => gc(db_path, days).await,
    }
}

async fn list(db_path: &Path, limit: i64) -> Result<()> {
    let db = InstalledDb::open(db_path)
        .await
        .context("open database")?;
    let runs = list_runs(&db, limit)
        .await
        .map_err(|e| anyhow::anyhow!("list runs: {}", e))?;

    if runs.is_empty() {
        println!("(no runs recorded)");
        return Ok(());
    }
    println!(
        "{:<33} {:<10} {:<13} {:<10} {}",
        "RUN", "KIND", "STATUS", "STARTED", "WORKFLOW"
    );
    for r in runs {
        let status = match r.terminal_status {
            Some(TerminalStatus::Succeeded) => "succeeded",
            Some(TerminalStatus::Failed) => "failed",
            Some(TerminalStatus::Aborted) => "aborted",
            None => "running",
        };
        println!(
            "{:<33} {:<10} {:<13} {:<10} {}",
            r.run_id,
            r.workflow_kind,
            status,
            humantime_ms(r.started_at),
            r.workflow_id.short()
        );
    }
    Ok(())
}

async fn show(db_path: &Path, run_prefix: &str) -> Result<()> {
    let db = InstalledDb::open(db_path)
        .await
        .context("open database")?;

    // Resolve a possibly-shortened run id to the full id by scanning recent runs.
    let recent = list_runs(&db, 200)
        .await
        .map_err(|e| anyhow::anyhow!("list runs: {}", e))?;
    let run = recent
        .into_iter()
        .find(|r| r.run_id.as_str().starts_with(run_prefix))
        .ok_or_else(|| anyhow::anyhow!("no run matching prefix {}", run_prefix))?;

    let _run_id_full = RunId::from_string(run.run_id.as_str().to_string());

    let steps = list_steps(&db, &run.workflow_id)
        .await
        .map_err(|e| anyhow::anyhow!("list steps: {}", e))?;

    println!("Run        : {}", run.run_id);
    println!("Workflow   : {} ({})", run.workflow_id, run.workflow_kind);
    println!(
        "Status     : {}",
        match run.terminal_status {
            Some(TerminalStatus::Succeeded) => "succeeded",
            Some(TerminalStatus::Failed) => "failed",
            Some(TerminalStatus::Aborted) => "aborted",
            None => "running",
        }
    );
    println!();
    println!(
        "{:<14} {:<14} {:<12} {:<8} {}",
        "KIND", "STATUS", "STARTED", "ATTEMPT", "STEP"
    );
    for s in steps {
        let started = s
            .started_at
            .map(humantime_ms)
            .unwrap_or_else(|| "-".to_string());
        let label = match s.status {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Succeeded => "succeeded",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
        };
        println!(
            "{:<14} {:<14} {:<12} {:<8} {}",
            s.kind,
            label,
            started,
            s.attempt,
            s.id.short()
        );
        if let (Status::Failed, Some(err)) = (s.status, s.error_text.as_deref()) {
            for line in err.lines().take(8) {
                println!("    {}", line);
            }
        }
    }
    Ok(())
}

async fn gc(db_path: &Path, days: i64) -> Result<()> {
    let db = InstalledDb::open(db_path)
        .await
        .context("open database")?;
    let removed = gc_workflows(&db, days * 24 * 3600 * 1000)
        .await
        .map_err(|e| anyhow::anyhow!("gc: {}", e))?;
    println!("removed {} workflow(s) older than {} days", removed, days);
    Ok(())
}

fn humantime_ms(ms: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let secs = (now - ms) / 1000;
    if secs < 60 {
        format!("{}s ago", secs.max(0))
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}
