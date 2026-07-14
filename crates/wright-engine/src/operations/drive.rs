//! Direct DAG execution — no persistent workflow state.
//!
//! Resume is handled by forge checkpoints (file-system sentinels keyed by
//! plan fingerprint) and deploy idempotence (database state). This layer
//! only schedules dependency batches and reports failures.

use std::path::Path;
use std::sync::Arc;

use crate::error::{Result, WrightError};
use futures_util::stream::{self, StreamExt};
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tracing::{error, info};

use crate::config::GlobalConfig;
use crate::resolve::BuildExecutionPlan;

pub struct DriveOptions<'a> {
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub quiet: bool,
}

/// Drive a forge plan to completion, executing tasks batch-by-batch.
///
/// No persistent workflow state — resume is handled entirely by the foundry
/// checkpoints (file-system sentinels keyed by plan fingerprint).
///
/// `concurrency` limits how many tasks within a batch run at once.
pub async fn drive_batches<F, Fut>(
    plan: &BuildExecutionPlan,
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

    for (batch_idx, batch) in plan.batches().iter().enumerate() {
        if *cancel.borrow() {
            return Err(WrightError::ForgeError("cancelled by user".into()));
        }

        let current_batch = batch_idx + 1;
        if !options.quiet {
            info!(
                event = "batch.started",
                batch_num = current_batch,
                total_batches = total_batches,
                task_count = batch.len(),
                "Build batch started"
            );
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
                    error!(event = "batch.failed", batch_num = batch_idx + 1, total_batches = total_batches, error = %e, "Build batch failed");
                    return Err(e);
                }
            }
        }
    }

    if !options.quiet {
        info!(
            event = "batch.all_completed",
            total_batches = total_batches,
            "All batches completed"
        );
    }

    Ok(())
}
