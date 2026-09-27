//! Direct DAG execution — no persistent workflow state.
//!
//! Resume is handled by forge checkpoints (file-system sentinels keyed by
//! plan fingerprint) and deploy idempotence (database state). This layer
//! only schedules dependency batches and reports failures.
//!
//! Failure semantics are cargo-style: tasks within one batch have no
//! inter-dependencies, so a failing task never interrupts its siblings.
//! A failure is announced the moment it happens while siblings are still
//! running (the failure that empties a batch is left to the terminal
//! failure report), every task runs to completion, and the failures are
//! settled together once no task is left running — a failed batch then
//! blocks the next one.

use std::path::Path;
use std::sync::Arc;

use crate::error::{BatchFailures, Result, TaskFailure, WrightError};
use futures_util::stream::{self, StreamExt};
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tracing::info;

use crate::config::GlobalConfig;
use crate::resolve::BuildExecutionPlan;
use crate::util::logging::report_task_failure;

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

        settle_batch(
            batch,
            current_batch,
            total_batches,
            concurrency,
            &semaphore,
            &task_fn,
            &cancel,
        )
        .await?;
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

/// Run one batch to completion and settle its failures.
///
/// `buffer_unordered` yields results in completion order, so a failure
/// while siblings are still running is announced the moment it happens
/// rather than when its predecessors finish; the failure that empties the
/// batch skips the notice, since the terminal failure report follows
/// immediately and would repeat it. The stream is always drained — a
/// failing task never cancels its siblings — then the batch settles:
/// cancellation wins over failure, a single failure keeps the legacy
/// terminal report, and multiple failures aggregate into [`BatchFailures`].
pub(crate) async fn settle_batch<F, Fut>(
    batch: &[String],
    batch_num: usize,
    total_batches: usize,
    concurrency: usize,
    semaphore: &Arc<Semaphore>,
    task_fn: &Arc<tokio::sync::Mutex<F>>,
    cancel: &watch::Receiver<bool>,
) -> Result<()>
where
    F: FnMut(String) -> Fut + Send,
    Fut: std::future::Future<Output = Result<()>> + Send,
{
    let mut stream = stream::iter(batch.iter().cloned())
        .map(|task| {
            let sem = semaphore.clone();
            let f = task_fn.clone();
            async move {
                let result: Result<()> = async {
                    let _permit = sem
                        .acquire()
                        .await
                        .map_err(|e| WrightError::context("semaphore", e))?;
                    let fut = {
                        let mut guard = f.lock().await;
                        guard(task.clone())
                    };
                    fut.await
                }
                .await;
                (task, result)
            }
        })
        .buffer_unordered(concurrency.max(1));

    let mut failures: Vec<TaskFailure> = Vec::new();
    let mut cancelled = false;
    let mut pending = batch.len();

    while let Some((task, result)) = stream.next().await {
        pending -= 1;
        match result {
            Ok(()) => {}
            Err(error) => {
                // A task failing because we reaped it on Ctrl-C is a
                // cancellation, not a build error — swallow it in favour of
                // the single "cancelled by user" outcome at batch end.
                if *cancel.borrow() {
                    cancelled = true;
                    continue;
                }
                // Announce immediately only while siblings are still
                // running; the failure that empties the batch is carried
                // by the terminal failure report alone — announcing both
                // would print the same failure twice.
                if pending > 0 {
                    report_task_failure(&task, false, &error);
                }
                failures.push(TaskFailure::failed(task, error));
            }
        }
    }

    if cancelled {
        return Err(WrightError::ForgeError("cancelled by user".into()));
    }

    if !failures.is_empty() {
        info!(
            event = "batch.failed",
            batch_num,
            total_batches,
            failed_tasks = failures.len(),
            "Build batch failed"
        );
    }

    BatchFailures::settle(batch_num, total_batches, failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    type SharedTaskFn<F> = Arc<tokio::sync::Mutex<F>>;

    fn harness<F, Fut>(task_fn: F) -> (Arc<Semaphore>, SharedTaskFn<F>, watch::Receiver<bool>)
    where
        F: FnMut(String) -> Fut + Send,
        Fut: std::future::Future<Output = Result<()>> + Send,
    {
        let (_tx, rx) = watch::channel(false);
        (
            Arc::new(Semaphore::new(8)),
            Arc::new(tokio::sync::Mutex::new(task_fn)),
            rx,
        )
    }

    #[tokio::test]
    async fn failing_task_does_not_interrupt_siblings() {
        let completed = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&completed);
        let (sem, f, rx) = harness(move |task: String| {
            let counter = Arc::clone(&counter);
            async move {
                if task == "b" {
                    // Fail fast; the slower siblings must still finish.
                    return Err(WrightError::ForgeError("boom".into()));
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        let batch = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let err = settle_batch(&batch, 1, 1, 8, &sem, &f, &rx)
            .await
            .unwrap_err();

        assert_eq!(completed.load(Ordering::SeqCst), 2);
        // A single failure keeps the legacy report text, with the chain
        // preserved for the terminal report.
        assert_eq!(err.to_string(), "task 'b' failed: forge error: boom");
    }

    #[tokio::test]
    async fn multiple_failures_aggregate_into_batch_error() {
        let completed = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&completed);
        let (sem, f, rx) = harness(move |task: String| {
            let counter = Arc::clone(&counter);
            async move {
                match task.as_str() {
                    "a" => return Err(WrightError::ForgeError("boom a".into())),
                    "c" => return Err(WrightError::ForgeError("boom c".into())),
                    _ => {}
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });

        let batch = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let err = settle_batch(&batch, 2, 3, 8, &sem, &f, &rx)
            .await
            .unwrap_err();

        assert_eq!(completed.load(Ordering::SeqCst), 1);
        let WrightError::BatchFailures(batch) = err else {
            panic!("expected BatchFailures, got {err:?}");
        };
        assert_eq!(batch.batch_num, 2);
        assert_eq!(batch.total_batches, 3);
        assert_eq!(batch.failures.len(), 2);
        let tasks: Vec<&str> = batch.failures.iter().map(|f| f.task.as_str()).collect();
        assert_eq!(tasks, ["a", "c"]);
        assert_eq!(batch.to_string(), "2 tasks failed in batch 2/3");
    }

    #[tokio::test]
    async fn cancelled_failures_settle_as_user_cancellation() {
        // Simulate Ctrl-C having fired before the reaped tasks report back.
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        let sem = Arc::new(Semaphore::new(8));
        let f = Arc::new(tokio::sync::Mutex::new(|_task: String| async move {
            Err(WrightError::ForgeError("reaped".into()))
        }));

        let batch = vec!["a".to_string(), "b".to_string()];
        let err = settle_batch(&batch, 1, 2, 8, &sem, &f, &rx)
            .await
            .unwrap_err();

        let WrightError::ForgeError(msg) = err else {
            panic!("expected ForgeError, got {err:?}");
        };
        assert_eq!(msg, "cancelled by user");
    }

    #[tokio::test]
    async fn clean_batch_is_ok() {
        let (sem, f, rx) = harness(|_task: String| async { Ok(()) });
        let batch = vec!["a".to_string(), "b".to_string()];
        settle_batch(&batch, 1, 1, 8, &sem, &f, &rx).await.unwrap();
    }
}
