//! Shared boilerplate for top-level commands that drive a workflow:
//! open the database, optionally wipe prior state for `--fresh`, drive the
//! spec to terminal, and exit non-zero on failure with a useful pointer.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::workflow::runs::delete_workflow;
use crate::workflow::{drive, RunOutcome, SchedulerPolicy, TerminalStatus, WorkflowSpec};

pub struct DriveOptions<'a> {
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub fresh: bool,
    pub quiet: bool,
}

pub async fn drive_command(spec: WorkflowSpec, opts: DriveOptions<'_>) -> Result<RunOutcome> {
    let db = InstalledDb::open(opts.db_path)
        .await
        .context("open database")?;

    if opts.fresh {
        delete_workflow(&db, &spec.workflow_id)
            .await
            .map_err(|e| anyhow::anyhow!("--fresh: {}", e))?;
    }

    let resources = crate::builder::orchestrator::summarize_build_resources(opts.config);
    let policy = SchedulerPolicy {
        cpu_concurrency: resources.concurrent_tasks.max(1),
        network_concurrency: 4,
        fail_fast: true,
        max_attempts: Some(3),
    };

    let log_dir = opts.config.general.logs_dir.join("runs");
    std::fs::create_dir_all(&log_dir).ok();

    let (cancel_tx, cancel_rx) = watch::channel(false);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::warn!("received interrupt, cancelling workflow...");
        let _ = cancel_tx.send(true);
    });
    let db_arc = Arc::new(db);

    let outcome = drive(db_arc, spec, policy, log_dir, cancel_rx)
        .await
        .map_err(|e| anyhow::anyhow!("workflow: {}", e))?;

    match outcome.status {
        TerminalStatus::Succeeded => {
            if !opts.quiet {
                tracing::info!("run {} succeeded", outcome.run_id.as_str());
            }
        }
        TerminalStatus::Failed => {
            for (step_id, msg) in &outcome.failed {
                eprintln!("step {} failed: {}", step_id.short(), msg);
            }
            eprintln!(
                "run {} failed; rerun the same command to resume, or `wright runs show {}`",
                outcome.run_id, outcome.run_id
            );
            std::process::exit(1);
        }
        TerminalStatus::Aborted => {
            eprintln!("run {} aborted", outcome.run_id);
            std::process::exit(1);
        }
    }
    Ok(outcome)
}
