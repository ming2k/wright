use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{mpsc, watch, Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

use super::errors::{Result, WorkflowError};
use super::id::{RunId, StepId, WorkflowId};
use super::spec::WorkflowSpec;
use super::step::{ResourceClass, ScheduledStep, Status, StepContext, TerminalStatus};
use super::store::WorkflowStore;
use crate::database::InstalledDb;

/// Runtime knobs. Step-kind concurrency is enforced via per-class semaphores;
/// `RootMutator` is always serialized.
#[derive(Clone, Copy, Debug)]
pub struct SchedulerPolicy {
    pub cpu_concurrency: usize,
    pub network_concurrency: usize,
    /// On the first failed step, drain in-flight work and stop launching new
    /// steps. Set false to fail-soft (record the failure, keep going on
    /// independent branches). Defaults to true; the build orchestrator
    /// behaves the same way today.
    pub fail_fast: bool,
    /// Maximum attempts per step across runs.  A step whose `attempt` count
    /// reaches this limit is treated as permanently failed and will not be
    /// re-attempted on future runs.  `None` means unlimited retries.
    pub max_attempts: Option<u32>,
}

impl Default for SchedulerPolicy {
    fn default() -> Self {
        Self {
            cpu_concurrency: 1,
            network_concurrency: 4,
            fail_fast: true,
            max_attempts: Some(3),
        }
    }
}

/// Result of `drive`. The DB has the source of truth; this is a convenience
/// summary for the caller.
#[derive(Debug)]
pub struct RunOutcome {
    pub run_id: RunId,
    pub workflow_id: WorkflowId,
    pub status: TerminalStatus,
    pub failed: Vec<(StepId, String)>,
}

struct StepDone {
    id: StepId,
    result: Result<serde_json::Value>,
}

#[allow(dead_code)]
enum Permit {
    Pool(OwnedSemaphorePermit),
    RootGuard(OwnedMutexGuard<()>),
    None,
}

/// Drive a workflow to a terminal state.
///
/// Idempotent across invocations: rerunning with the same `WorkflowSpec`
/// preserves succeeded steps and only re-attempts pending/failed work.
pub async fn drive(
    db: Arc<InstalledDb>,
    spec: WorkflowSpec,
    policy: SchedulerPolicy,
    log_dir: PathBuf,
    cancel: watch::Receiver<bool>,
) -> Result<RunOutcome> {
    let store = WorkflowStore::new(&db);

    store
        .upsert_workflow(&spec.workflow_id, spec.kind, &spec.inputs_json)
        .await?;
    for s in &spec.steps {
        store.upsert_step(&spec.workflow_id, s).await?;
    }

    // A previous process death may have left some rows in `running`. The
    // database lock guarantees no other process owns them, so reset to pending.
    store.reset_running(&spec.workflow_id).await?;

    let run_id = RunId::new();
    store.start_run(&run_id, &spec.workflow_id).await?;
    info!(
        workflow = %spec.workflow_id.short(),
        run = %run_id,
        kind = %spec.kind,
        steps = spec.steps.len(),
        "starting workflow run"
    );

    let cpu = Arc::new(Semaphore::new(policy.cpu_concurrency.max(1)));
    let net = Arc::new(Semaphore::new(policy.network_concurrency.max(1)));
    let root_mut: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // Index steps by id for cheap lookup. Move out of the spec so we can call
    // their `run` closures.
    let mut by_id: HashMap<StepId, ScheduledStep> =
        spec.steps.into_iter().map(|s| (s.id.clone(), s)).collect();
    let total_steps = by_id.len();

    let mut statuses: HashMap<StepId, Status> = store.load_statuses(&spec.workflow_id).await?;
    // Steps not yet in the DB don't show up in load_statuses; treat as pending.
    for id in by_id.keys() {
        statuses.entry(id.clone()).or_insert(Status::Pending);
    }

    let attempts: HashMap<StepId, i32> = store.load_attempts(&spec.workflow_id).await?;

    let max_attempts = policy.max_attempts.map(|n| n as i32);

    // Steps whose attempt count already hit the limit are permanently failed.
    let mut permanent_failures: std::collections::HashSet<StepId> =
        std::collections::HashSet::new();
    for (id, status) in &statuses {
        if *status == Status::Failed {
            if let Some(limit) = max_attempts {
                let att = attempts.get(id).copied().unwrap_or(0);
                if att >= limit {
                    permanent_failures.insert(id.clone());
                }
            }
        }
    }

    let outputs: HashMap<StepId, serde_json::Value> = store.load_outputs(&spec.workflow_id).await?;
    // Outputs are read-only in the runner once loaded; in-flight steps push
    // their results through the channel, and the loop owns the merged map.
    let mut outputs = outputs;

    let (tx, mut rx) = mpsc::channel::<StepDone>(total_steps.max(1));
    let mut in_flight: usize = 0;
    let mut failed: Vec<(StepId, String)> = Vec::new();
    let mut stop_launching = false;

    loop {
        // Cancellation: stop launching, drain in-flight.
        if *cancel.borrow() {
            stop_launching = true;
        }

        // 1. Schedule everything ready that we have capacity for. We try every
        //    candidate every iteration because a Cpu pool slot may free up
        //    independently of the step that just finished.
        if !stop_launching {
            let ready_ids: Vec<StepId> = by_id
                .keys()
                .filter(|id| {
                    if permanent_failures.contains(*id) {
                        return false;
                    }
                    let st = statuses.get(*id).copied().unwrap_or(Status::Pending);
                    if !matches!(st, Status::Pending | Status::Failed) {
                        return false;
                    }
                    let s = &by_id[*id];
                    s.depends_on
                        .iter()
                        .all(|d| matches!(statuses.get(d), Some(Status::Succeeded)))
                })
                .cloned()
                .collect();

            for id in ready_ids {
                let permit = match by_id[&id].class {
                    ResourceClass::Cpu => match cpu.clone().try_acquire_owned() {
                        Ok(p) => Permit::Pool(p),
                        Err(_) => continue,
                    },
                    ResourceClass::Network => match net.clone().try_acquire_owned() {
                        Ok(p) => Permit::Pool(p),
                        Err(_) => continue,
                    },
                    ResourceClass::RootMutator => match root_mut.clone().try_lock_owned() {
                        Ok(g) => Permit::RootGuard(g),
                        Err(_) => continue,
                    },
                    ResourceClass::Trivial => Permit::None,
                };

                let step = by_id.remove(&id).expect("ready id present");

                // Collect upstream outputs at launch time; the snapshot is
                // sufficient because deps must be `succeeded`.
                let upstream: HashMap<StepId, serde_json::Value> = step
                    .depends_on
                    .iter()
                    .filter_map(|d| outputs.get(d).map(|v| (d.clone(), v.clone())))
                    .collect();

                statuses.insert(step.id.clone(), Status::Running);
                store.mark_running(&step.id).await?;
                store
                    .record_event(&run_id, &step.id, "started", None)
                    .await?;
                let _ = store.heartbeat(&run_id).await;

                let ctx = StepContext {
                    run_id: run_id.clone(),
                    workflow_id: spec.workflow_id.clone(),
                    step_id: step.id.clone(),
                    db: db.clone(),
                    log_dir: log_dir.clone(),
                    cancel: cancel.clone(),
                    upstream_outputs: upstream,
                };

                in_flight += 1;
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _permit = permit; // released on drop, after step finishes
                    let result = (step.run)(ctx).await;
                    let _ = tx
                        .send(StepDone {
                            id: step.id,
                            result,
                        })
                        .await;
                });
            }
        }

        // 2. Termination conditions.
        if in_flight == 0 {
            let any_pending = statuses.iter().any(|(id, s)| {
                matches!(s, Status::Pending | Status::Failed) && !permanent_failures.contains(id)
            });
            if !any_pending || stop_launching {
                break;
            }
            // Pending exist but nothing in flight and nothing was launched =>
            // remaining steps have unmet deps (cycle, or dep on a permanently-
            // failed step). Report and exit.
            let unmet = describe_unmet_deps(&by_id_keys(&statuses, &outputs), &statuses);
            return Err(WorkflowError::Deadlock(unmet));
        }

        // 3. Wait for one completion.
        let Some(done) = rx.recv().await else {
            return Err(WorkflowError::ChannelClosed);
        };
        in_flight -= 1;
        match done.result {
            Ok(out) => {
                statuses.insert(done.id.clone(), Status::Succeeded);
                outputs.insert(done.id.clone(), out.clone());
                store.mark_succeeded(&done.id, &out).await?;
                store
                    .record_event(&run_id, &done.id, "succeeded", None)
                    .await?;
                let _ = store.heartbeat(&run_id).await;
            }
            Err(e) => {
                let msg = format!("{:#}", e);
                warn!(step = %done.id.short(), error = %msg, "step failed");
                statuses.insert(done.id.clone(), Status::Failed);
                store.mark_failed(&done.id, &msg).await?;
                store
                    .record_event(&run_id, &done.id, "failed", Some(&msg))
                    .await?;
                failed.push((done.id, msg));
                if policy.fail_fast {
                    stop_launching = true;
                }
            }
        }
    }

    // Drain any remaining in-flight after we stopped launching.
    while in_flight > 0 {
        match rx.recv().await {
            Some(done) => {
                in_flight -= 1;
                match done.result {
                    Ok(out) => {
                        statuses.insert(done.id.clone(), Status::Succeeded);
                        store.mark_succeeded(&done.id, &out).await?;
                        store
                            .record_event(&run_id, &done.id, "succeeded", None)
                            .await?;
                    }
                    Err(e) => {
                        let msg = format!("{:#}", e);
                        statuses.insert(done.id.clone(), Status::Failed);
                        store.mark_failed(&done.id, &msg).await?;
                        store
                            .record_event(&run_id, &done.id, "failed", Some(&msg))
                            .await?;
                        failed.push((done.id, msg));
                    }
                }
            }
            None => break,
        }
    }

    for id in &permanent_failures {
        let att = attempts.get(id).copied().unwrap_or(0);
        let msg = format!(
            "permanently failed after {} attempt{}",
            att,
            if att == 1 { "" } else { "s" }
        );
        failed.push((id.clone(), msg));
    }

    let cancelled = *cancel.borrow();
    let terminal = if cancelled {
        TerminalStatus::Aborted
    } else if failed.is_empty() {
        TerminalStatus::Succeeded
    } else {
        TerminalStatus::Failed
    };
    store.finish_run(&run_id, terminal).await?;

    Ok(RunOutcome {
        run_id,
        workflow_id: spec.workflow_id,
        status: terminal,
        failed,
    })
}

/// Used only for diagnostic Deadlock messages; cheap and rare.
fn by_id_keys(
    statuses: &HashMap<StepId, Status>,
    _outputs: &HashMap<StepId, serde_json::Value>,
) -> Vec<StepId> {
    statuses.keys().cloned().collect()
}

fn describe_unmet_deps(all: &[StepId], statuses: &HashMap<StepId, Status>) -> String {
    let mut lines: Vec<String> = all
        .iter()
        .filter(|id| {
            matches!(
                statuses.get(*id).copied().unwrap_or(Status::Pending),
                Status::Pending | Status::Failed
            )
        })
        .map(|id| {
            format!(
                "{} ({:?})",
                id.short(),
                statuses.get(id).copied().unwrap_or(Status::Pending)
            )
        })
        .collect();
    lines.sort();
    lines.join(", ")
}
