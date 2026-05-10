//! Shared boilerplate for top-level commands that drive a workflow.
//!
//! Core design: workflow state is a cache of execution progress.  `--invalidate`
//! tells the engine "discard cached progress for these inputs and re-compute".
//! There is no separate `--restart` or `--fresh`; a single invalidation flag is
//! sufficient because the engine always converges deterministically from state.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::watch;

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::util::progress::SUPPRESS_INFO_LOGS;
use crate::workflow::{drive, RunOutcome, SchedulerPolicy, TerminalStatus, WorkflowSpec};
use std::sync::atomic::Ordering;

pub struct DriveOptions<'a> {
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    /// Discard any persisted workflow progress for this spec and re-execute
    /// from scratch.  This is the single, unambiguous knob for "I want a clean
    /// re-run" — build-stage and install caches downstream are still governed
    /// by their own content-addressed checks.
    pub invalidate: bool,
    pub quiet: bool,
}

pub async fn drive_command(spec: WorkflowSpec, opts: DriveOptions<'_>) -> Result<RunOutcome> {
    let db = InstalledDb::open(opts.db_path)
        .await
        .context("open database")?;

    if opts.invalidate {
        crate::workflow::WorkflowStore::new(&db)
            .clear_workflow(&spec.workflow_id)
            .await
            .map_err(|e| anyhow::anyhow!("--invalidate: {}", e))?;
    }

    let resources = crate::planning::summarize_build_resources(opts.config);
    let policy = SchedulerPolicy {
        cpu_concurrency: resources.concurrent_tasks.max(1),
        network_concurrency: 4,
        fail_fast: true,
        max_attempts: Some(3),
    };

    let log_dir = opts.config.general.logs_dir.join("workflow");
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

    // Reset log suppression so subsequent commands are not affected.
    SUPPRESS_INFO_LOGS.store(false, Ordering::Relaxed);

    match outcome.status {
        TerminalStatus::Succeeded => {
            if !opts.quiet {
                tracing::info!("workflow {} succeeded", outcome.workflow_id.short());
            }
        }
        TerminalStatus::Failed => {
            if !opts.quiet {
                eprintln!("\n=== failures summary ===");
                for (step_id, msg, plan, label) in &outcome.failed {
                    let ctx = match (plan, label) {
                        (Some(p), Some(l)) => format!(" [{} / {}]", p, l),
                        (Some(p), None) => format!(" [{}]", p),
                        (None, Some(l)) => format!(" [{}]", l),
                        (None, None) => String::new(),
                    };
                    eprintln!("  step {} failed{}: {}", step_id.short(), ctx, msg);
                }
                eprintln!(
                    "workflow {} failed; rerun the same command to resume, or use --invalidate to discard active workflow state",
                    outcome.workflow_id.short()
                );
            }
            std::process::exit(1);
        }
        TerminalStatus::Aborted => {
            eprintln!(
                "workflow {} aborted; rerun the same command to resume",
                outcome.workflow_id.short()
            );
            std::process::exit(1);
        }
    }
    Ok(outcome)
}
