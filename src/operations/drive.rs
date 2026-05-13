//! Direct DAG execution — no persistent workflow state.
//!
//! Resume is handled by forge checkpoints (file-system sentinels keyed by
//! plan fingerprint) and deploy idempotence (database state). This layer
//! only schedules dependency batches and reports failures.

use std::path::Path;
use std::sync::Arc;

use crate::error::{Result, WrightError};
use futures_util::stream::{self, StreamExt};
use indicatif::ProgressBar;
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tracing::{error, info};

use crate::config::GlobalConfig;
use crate::resolve::ForgeExecutionPlan;

pub struct DriveOptions<'a> {
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub quiet: bool,
    pub flow_progress: Option<ProgressBar>,
}

/// Drive a forge plan to completion, executing tasks batch-by-batch.
///
/// No persistent workflow state — resume is handled entirely by forger
/// checkpoints (file-system sentinels keyed by plan fingerprint).
///
/// `concurrency` limits how many tasks within a batch run at once.
pub async fn drive_batches<F, Fut>(
    plan: &ForgeExecutionPlan,
    options: &DriveOptions<'_>,
    concurrency: usize,
    task_fn: F,
    cancel: watch::Receiver<bool>,
) -> Result<()>
where
    F: FnMut(String) -> Fut + Send,
    Fut: std::future::Future<Output = Result<()>> + Send,
{
    let task_fn = Arc::new(tokio::sync::Mutex::new(task_fn));
    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let total_batches = plan.batches().len();
    let cancel = cancel;

    if let Some(ref flow) = options.flow_progress {
        flow.set_message(format!(
            "batch 1/{}: {} plan{}",
            total_batches,
            plan.batches().first().map_or(0, |b| b.len()),
            if plan.batches().first().map_or(0, |b| b.len()) == 1 {
                ""
            } else {
                "s"
            }
        ));
    }

    for (batch_idx, batch) in plan.batches().iter().enumerate() {
        if *cancel.borrow() {
            return Err(WrightError::ForgeError("cancelled by user".into()));
        }

        if !options.quiet {
            info!(
                "batch {}/{}: {} task(s)",
                batch_idx + 1,
                total_batches,
                batch.len()
            );
        }

        if batch_idx > 0
            && let Some(ref flow) = options.flow_progress {
                flow.set_message(format!(
                    "batch {}/{}: {} plan{}",
                    batch_idx + 1,
                    total_batches,
                    batch.len(),
                    if batch.len() == 1 { "" } else { "s" }
                ));
            }

        let results: Vec<Result<()>> = stream::iter(batch.iter().cloned())
            .map(|task| {
                let sem = semaphore.clone();
                let f = task_fn.clone();
                async move {
                    let _permit = sem
                        .acquire()
                        .await
                        .map_err(|e| WrightError::ForgeError(format!("semaphore: {}", e)))?;
                    let fut = {
                        let mut guard = f.lock().await;
                        guard(task)
                    };
                    fut.await
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        for result in results {
            match result {
                Ok(()) => {}
                Err(e) => {
                    if let Some(ref flow) = options.flow_progress {
                        flow.set_message(format!(
                            "batch {}/{}: aborted",
                            batch_idx + 1,
                            total_batches
                        ));
                    }
                    error!("batch {}/{} failed: {:#}", batch_idx + 1, total_batches, e);
                    return Err(e);
                }
            }
        }
    }

    if let Some(ref flow) = options.flow_progress {
        flow.set_message("complete".to_string());
    }

    if !options.quiet {
        info!("all batches completed");
    }

    Ok(())
}
