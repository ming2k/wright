use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::util::progress::SUPPRESS_INFO_LOGS;
use std::sync::atomic::Ordering;
use tokio::sync::{mpsc, watch, Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};
use tracing::{info, warn};

use super::errors::{Result, WorkflowError};
use super::id::{StepId, WorkflowId};
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
    /// independent branches). Defaults to true; the planning layer
    /// behaves the same way today.
    pub fail_fast: bool,
    /// Maximum attempts per step across drive attempts.  A step whose `attempt` count
    /// reaches this limit is treated as permanently failed and will not be
    /// re-attempted on future attempts.  `None` means unlimited retries.
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
    pub workflow_id: WorkflowId,
    pub status: TerminalStatus,
    /// (step_id, message, plan_name, label)
    pub failed: Vec<(StepId, String, Option<String>, Option<String>)>,
}

struct StepDone {
    id: StepId,
    kind: &'static str,
    plan_name: Option<String>,
    label: Option<String>,
    inputs_json: String,
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
/// Idempotent while a workflow is incomplete: rerunning with the same
/// `WorkflowSpec` preserves succeeded upstream steps until the workflow
/// reaches success. Successful workflows clear their step state before return.
pub async fn drive(
    db: Arc<InstalledDb>,
    spec: WorkflowSpec,
    policy: SchedulerPolicy,
    log_dir: PathBuf,
    mut cancel: watch::Receiver<bool>,
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

    info!(
        workflow = %spec.workflow_id.short(),
        kind = %spec.kind,
        steps = spec.steps.len(),
        "starting workflow"
    );

    let cpu = Arc::new(Semaphore::new(policy.cpu_concurrency.max(1)));
    let net = Arc::new(Semaphore::new(policy.network_concurrency.max(1)));
    let root_mut: Arc<Mutex<()>> = Arc::new(Mutex::new(()));

    // Index steps by id for cheap lookup. Move out of the spec so we can call
    // their `run` closures.
    let mut by_id: HashMap<StepId, ScheduledStep> =
        spec.steps.into_iter().map(|s| (s.id.clone(), s)).collect();
    let total_steps = by_id.len();

    // Keep lightweight metadata for diagnostics after steps are removed from `by_id`.
    let step_metadata: HashMap<StepId, (Option<String>, Option<String>, Vec<StepId>)> = by_id
        .iter()
        .map(|(id, s)| (id.clone(), (s.plan_name.clone(), s.label.clone(), s.depends_on.clone())))
        .collect();

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
    let mut failed: Vec<(StepId, String, Option<String>, Option<String>)> = Vec::new();
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
                let ctx = StepContext {
                    workflow_id: spec.workflow_id.clone(),
                    step_id: step.id.clone(),
                    db: db.clone(),
                    log_dir: log_dir.clone(),
                    cancel: cancel.clone(),
                    upstream_outputs: upstream,
                };

                in_flight += 1;
                let tx = tx.clone();
                let done_id = step.id.clone();
                let done_kind = step.kind;
                let done_plan_name = step.plan_name.clone();
                let done_label = step.label.clone();
                let done_inputs_json = step.inputs_json.clone();
                tokio::spawn(async move {
                    let _permit = permit; // released on drop, after step finishes
                    let result = (step.run)(ctx).await;
                    let _ = tx
                        .send(StepDone {
                            id: done_id,
                            kind: done_kind,
                            plan_name: done_plan_name,
                            label: done_label,
                            inputs_json: done_inputs_json,
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
            let failures = store.load_failures(&spec.workflow_id).await.unwrap_or_default();
            let unmet = describe_unmet_deps(
                &statuses,
                &step_metadata,
                &failures,
                &permanent_failures,
            );
            return Err(WorkflowError::BlockedByFailures(unmet));
        }

        // 3. Wait for one completion, but also honour cancellation.
        let done = tokio::select! {
            Some(done) = rx.recv() => done,
            _ = wait_cancelled(&mut cancel) => break,
        };
        in_flight -= 1;
        match done.result {
            Ok(out) => {
                statuses.insert(done.id.clone(), Status::Succeeded);
                outputs.insert(done.id.clone(), out.clone());
                store.mark_succeeded(&done.id, &out).await?;
            }
            Err(e) => {
                let msg = format!("{:#}", e);
                let plan = done.plan_name.clone();
                let label = done.label.clone();
                statuses.insert(done.id.clone(), Status::Failed);
                let failure = failure_json(done.kind, &done.inputs_json, &msg);
                store.mark_failed(&done.id, &failure).await?;
                failed.push((done.id.clone(), msg.clone(), plan.clone(), label.clone()));
                if policy.fail_fast {
                    let first_failure = !stop_launching;
                    stop_launching = true;
                    if first_failure {
                        SUPPRESS_INFO_LOGS.store(true, Ordering::Relaxed);
                        let ctx = match (&plan, &label) {
                            (Some(p), Some(l)) => format!(" [{} / {}]", p, l),
                            (Some(p), None) => format!(" [{}]", p),
                            (None, Some(l)) => format!(" [{}]", l),
                            (None, None) => String::new(),
                        };
                        warn!("step {} failed{}: {}", done.id.short(), ctx, msg);
                        if in_flight > 0 {
                            warn!("halting: no new steps will be launched, waiting for {} already-running task(s) to finish", in_flight - 1);
                        }
                    }
                }
            }
        }
    }

    // Drain any remaining in-flight after we stopped launching.
    while in_flight > 0 {
        let done = tokio::select! {
            Some(done) = rx.recv() => done,
            _ = wait_cancelled(&mut cancel) => break,
        };
        in_flight -= 1;
        match done.result {
            Ok(out) => {
                statuses.insert(done.id.clone(), Status::Succeeded);
                store.mark_succeeded(&done.id, &out).await?;
            }
            Err(e) => {
                let msg = format!("{:#}", e);
                let plan = done.plan_name.clone();
                let label = done.label.clone();
                statuses.insert(done.id.clone(), Status::Failed);
                let failure = failure_json(done.kind, &done.inputs_json, &msg);
                store.mark_failed(&done.id, &failure).await?;
                failed.push((done.id, msg, plan, label));
            }
        }
    }

    for id in &permanent_failures {
        let att = attempts.get(id).copied().unwrap_or(0);
        let msg = format!(
            "permanently failed after {} attempt{}",
            att,
            if att == 1 { "" } else { "s" }
        );
        failed.push((id.clone(), msg, None, None));
    }

    let cancelled = *cancel.borrow();
    let terminal = if cancelled {
        TerminalStatus::Aborted
    } else if failed.is_empty() {
        TerminalStatus::Succeeded
    } else {
        TerminalStatus::Failed
    };
    if terminal == TerminalStatus::Succeeded {
        store.clear_workflow(&spec.workflow_id).await?;
    }

    Ok(RunOutcome {
        workflow_id: spec.workflow_id,
        status: terminal,
        failed,
    })
}

async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn describe_unmet_deps(
    statuses: &HashMap<StepId, Status>,
    step_metadata: &HashMap<StepId, (Option<String>, Option<String>, Vec<StepId>)>,
    failures: &HashMap<StepId, serde_json::Value>,
    permanent_failures: &std::collections::HashSet<StepId>,
) -> String {
    use std::fmt::Write;

    let mut failed_steps: Vec<(StepId, String)> = Vec::new();
    let mut pending_steps: Vec<(StepId, Vec<StepId>)> = Vec::new();

    for (id, status) in statuses {
        if permanent_failures.contains(id) {
            let name = step_display_name(id, step_metadata.get(id));
            failed_steps.push((
                id.clone(),
                format!("{} — permanently failed (max attempts reached)", name),
            ));
        } else if *status == Status::Failed {
            let name = step_display_name(id, step_metadata.get(id));
            let err_msg = failures
                .get(id)
                .and_then(|f| f.get("message").and_then(|m| m.as_str()))
                .unwrap_or("unknown error");
            failed_steps.push((
                id.clone(),
                format!("{} — {}", name, err_msg),
            ));
        } else if *status == Status::Pending {
            let deps = step_metadata
                .get(id)
                .map(|m| &m.2)
                .cloned()
                .unwrap_or_default();
            let blocked_by: Vec<StepId> = deps
                .into_iter()
                .filter(|d| {
                    matches!(
                        statuses.get(d).copied().unwrap_or(Status::Pending),
                        Status::Failed
                    ) || permanent_failures.contains(d)
                })
                .collect();
            if !blocked_by.is_empty() {
                pending_steps.push((id.clone(), blocked_by));
            }
        }
    }

    let mut out = String::new();

    if !failed_steps.is_empty() {
        writeln!(&mut out, "\n=== failed steps ({} total) ===", failed_steps.len()).ok();
        for (_, msg) in &failed_steps {
            writeln!(&mut out, "  {}", msg).ok();
        }
    }

    if !pending_steps.is_empty() {
        writeln!(
            &mut out,
            "\n=== pending steps blocked by failed dependencies ({} total) ===",
            pending_steps.len()
        )
        .ok();
        for (id, blocked_by) in &pending_steps {
            let name = step_display_name(id, step_metadata.get(id));
            let blockers: Vec<String> = blocked_by
                .iter()
                .map(|b| {
                    let b_name = step_display_name(b, step_metadata.get(b));
                    format!("{} ({})", b_name, b.short())
                })
                .collect();
            writeln!(&mut out, "  {} blocked by: {}", name, blockers.join(", ")).ok();
        }
    }

    if out.is_empty() {
        out = "no pending or failed steps found (possible cycle)".to_string();
    } else {
        write!(
            &mut out,
            "\nuse --invalidate to discard failed workflow state and retry from scratch"
        )
        .ok();
    }

    out
}

fn step_display_name(
    id: &StepId,
    meta: Option<&(Option<String>, Option<String>, Vec<StepId>)>,
) -> String {
    let (plan, label) = meta
        .map(|m| (m.0.as_deref(), m.1.as_deref()))
        .unwrap_or((None, None));
    match (plan, label) {
        (Some(p), Some(l)) => format!("{} [{} / {}]", id.short(), p, l),
        (Some(p), None) => format!("{} [{}]", id.short(), p),
        (None, Some(l)) => format!("{} [{}]", id.short(), l),
        (None, None) => id.short().to_string(),
    }
}

fn failure_json(kind: &str, inputs_json: &str, message: &str) -> serde_json::Value {
    let inputs = serde_json::from_str::<serde_json::Value>(inputs_json)
        .unwrap_or_else(|_| serde_json::Value::Null);
    let mut obj = serde_json::Map::new();
    obj.insert(
        "reason".to_string(),
        serde_json::Value::String(reason_for(kind, message)),
    );
    obj.insert(
        "message".to_string(),
        serde_json::Value::String(bounded_message(kind, message)),
    );

    if let Some(plan) = inputs
        .get("plan")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
    {
        obj.insert(
            "plan".to_string(),
            serde_json::Value::String(plan.to_string()),
        );
    }
    if let Some(label) = inputs.get("label").and_then(|v| v.as_str()) {
        obj.insert(
            "label".to_string(),
            serde_json::Value::String(label.to_string()),
        );
    }
    if let Some(root_dir) = inputs.get("root_dir").and_then(|v| v.as_str()) {
        obj.insert(
            "root_dir".to_string(),
            serde_json::Value::String(root_dir.to_string()),
        );
    }
    if let Some(stage) = extract_between(message, "stage '", "'") {
        obj.insert("stage".to_string(), serde_json::Value::String(stage));
    }
    if let Some(exit_status) = extract_exit_status(message) {
        obj.insert(
            "exit_status".to_string(),
            serde_json::Value::Number(exit_status.into()),
        );
    }

    serde_json::Value::Object(obj)
}

fn reason_for(kind: &str, message: &str) -> String {
    match kind {
        "build_plan" if message.contains("stage '") && message.contains("exit code") => {
            "stage_failed"
        }
        "package_plan" if message.contains("expected archive not produced") => "archive_missing",
        "install_batch" if message.contains("conflict") => "file_conflict",
        _ => "step_failed",
    }
    .to_string()
}

fn bounded_message(kind: &str, message: &str) -> String {
    let msg = if kind == "build_plan" {
        if let Some(stage) = extract_between(message, "stage '", "'") {
            if let Some(code) = extract_exit_status(message) {
                format!("stage '{}' failed with exit status {}", stage, code)
            } else {
                format!("stage '{}' failed", stage)
            }
        } else {
            first_non_log_line(message)
        }
    } else {
        first_non_log_line(message)
    };
    truncate_chars(&msg, 512)
}

fn first_non_log_line(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("Log: "))
        .unwrap_or("step failed")
        .to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in s.chars().take(max) {
        out.push(ch);
    }
    if s.chars().count() > max {
        out.push_str("...");
    }
    out
}

fn extract_between(message: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = message.find(start)? + start.len();
    let rest = &message[start_idx..];
    let end_idx = rest.find(end)?;
    Some(rest[..end_idx].to_string())
}

fn extract_exit_status(message: &str) -> Option<i64> {
    let marker = "exit code ";
    let idx = message.find(marker)? + marker.len();
    let digits: String = message[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}
