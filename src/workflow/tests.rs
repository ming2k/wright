//! Foundation tests for the workflow runner.
//!
//! These tests exercise the runner against synthetic in-memory steps;
//! real step kinds (BuildPlan, PackagePlan, …) are covered by their own
//! integration tests once they land.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::*;
use crate::database::InstalledDb;

/// A test step that records each invocation, then succeeds with a payload.
struct CountingStep {
    inputs: TestInputs,
    deps: Vec<StepId>,
    counter: Arc<AtomicUsize>,
    fail_first_n: usize,
}

#[derive(Serialize, Deserialize, Clone)]
struct TestInputs {
    name: String,
}

#[derive(Serialize, Deserialize)]
struct TestOutputs {
    name: String,
    invocation: usize,
}

impl Step for CountingStep {
    type Inputs = TestInputs;
    type Outputs = TestOutputs;
    const KIND: &'static str = "test_counting";
    const RESOURCE_CLASS: ResourceClass = ResourceClass::Trivial;

    fn inputs(&self) -> &Self::Inputs {
        &self.inputs
    }

    fn depends_on(&self) -> &[StepId] {
        &self.deps
    }

    fn execute(self: Arc<Self>, _ctx: StepContext) -> BoxFuture<'static, Result<Self::Outputs>> {
        Box::pin(async move {
            let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= self.fail_first_n {
                return Err(WorkflowError::StepFailed(format!(
                    "{} forced failure on invocation {}",
                    self.inputs.name, n
                )));
            }
            Ok(TestOutputs {
                name: self.inputs.name.clone(),
                invocation: n,
            })
        })
    }
}

fn make_step(
    name: &str,
    deps: Vec<StepId>,
    counter: Arc<AtomicUsize>,
    fail_first_n: usize,
) -> CountingStep {
    CountingStep {
        inputs: TestInputs {
            name: name.to_string(),
        },
        deps,
        counter,
        fail_first_n,
    }
}

async fn fresh_db() -> Arc<InstalledDb> {
    Arc::new(InstalledDb::open_in_memory().await.unwrap())
}

fn never_cancelled() -> watch::Receiver<bool> {
    let (_, rx) = watch::channel(false);
    rx
}

fn policy() -> SchedulerPolicy {
    SchedulerPolicy {
        cpu_concurrency: 4,
        network_concurrency: 4,
        fail_fast: true,
        max_attempts: Some(3),
    }
}

#[tokio::test]
async fn drives_a_simple_workflow_to_success() {
    let db = fresh_db().await;
    let counter = Arc::new(AtomicUsize::new(0));

    let mut b = WorkflowBuilder::new("test", &serde_json::json!({"case": "simple"})).unwrap();
    let a = b.add(make_step("a", vec![], counter.clone(), 0)).unwrap();
    let b_id = b
        .add(make_step("b", vec![a.clone()], counter.clone(), 0))
        .unwrap();
    let _c = b
        .add(make_step("c", vec![b_id], counter.clone(), 0))
        .unwrap();
    let spec = b.build();

    let outcome = drive(
        db.clone(),
        spec,
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.status, TerminalStatus::Succeeded);
    assert!(outcome.failed.is_empty());
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn clears_step_state_after_success() {
    let db = fresh_db().await;
    let counter = Arc::new(AtomicUsize::new(0));

    // Successful workflows do not keep step rows forever. Repeating the same
    // command should re-run the workflow layer and let lower-level idempotence
    // checks decide what can be skipped.
    let inputs = serde_json::json!({"case": "resume"});

    let make_spec = |counter: Arc<AtomicUsize>| -> WorkflowSpec {
        let mut b = WorkflowBuilder::new("test", &inputs).unwrap();
        let a = b.add(make_step("a", vec![], counter.clone(), 0)).unwrap();
        b.add(make_step("b", vec![a], counter.clone(), 0)).unwrap();
        b.build()
    };

    drive(
        db.clone(),
        make_spec(counter.clone()),
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();
    let after_first = counter.load(Ordering::SeqCst);
    assert_eq!(after_first, 2);
    let workflow_id = make_spec(counter.clone()).workflow_id;
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM workflow_steps WHERE workflow_id = ?")
        .bind(workflow_id.as_str())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "successful workflows should clear step rows");

    drive(
        db.clone(),
        make_spec(counter.clone()),
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();
    let after_second = counter.load(Ordering::SeqCst);
    assert_eq!(
        after_second, 4,
        "second drive should rebuild workflow step state after success"
    );
}

#[tokio::test]
async fn re_runs_failed_step_on_next_drive() {
    let db = fresh_db().await;
    let counter = Arc::new(AtomicUsize::new(0));

    let inputs = serde_json::json!({"case": "retry"});

    // Step `a` fails on its first invocation, succeeds on the second.
    let make_spec = |counter: Arc<AtomicUsize>, fail: usize| -> WorkflowSpec {
        let mut b = WorkflowBuilder::new("test", &inputs).unwrap();
        b.add(make_step("a", vec![], counter, fail)).unwrap();
        b.build()
    };

    let outcome = drive(
        db.clone(),
        make_spec(counter.clone(), 1),
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.status, TerminalStatus::Failed);
    assert_eq!(outcome.failed.len(), 1);

    // Rerun: the step is in `failed`, the runner should retry it.
    let outcome = drive(
        db.clone(),
        make_spec(counter.clone(), 0),
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.status, TerminalStatus::Succeeded);
    assert!(outcome.failed.is_empty());
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_step_records_structured_failure_json() {
    let db = fresh_db().await;
    let counter = Arc::new(AtomicUsize::new(0));

    let mut b = WorkflowBuilder::new("test", &serde_json::json!({"case": "failure-json"})).unwrap();
    let step_id = b.add(make_step("a", vec![], counter, 1)).unwrap();
    let outcome = drive(
        db.clone(),
        b.build(),
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.status, TerminalStatus::Failed);

    let raw: String = sqlx::query_scalar("SELECT failure_json FROM workflow_steps WHERE id = ?")
        .bind(step_id.as_str())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let failure: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(failure["reason"], "step_failed");
    assert!(failure["message"]
        .as_str()
        .unwrap()
        .contains("forced failure"));
}

#[tokio::test]
async fn does_not_run_dependents_of_failed_step() {
    let db = fresh_db().await;
    let counter = Arc::new(AtomicUsize::new(0));

    let mut b = WorkflowBuilder::new("test", &serde_json::json!({"case": "fail-cascade"})).unwrap();
    let a = b.add(make_step("a", vec![], counter.clone(), 99)).unwrap(); // always fails
    let _b_step = b
        .add(make_step("b", vec![a.clone()], counter.clone(), 0))
        .unwrap();
    let spec = b.build();

    let outcome = drive(
        db.clone(),
        spec,
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.status, TerminalStatus::Failed);
    // `a` ran (and failed); `b` did not run because its dep is not succeeded.
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn workflow_id_is_stable_across_builders() {
    let inputs = serde_json::json!({"x": 1, "y": 2});
    let counter = Arc::new(AtomicUsize::new(0));

    let mut b1 = WorkflowBuilder::new("test", &inputs).unwrap();
    b1.add(make_step("a", vec![], counter.clone(), 0)).unwrap();

    let mut b2 = WorkflowBuilder::new("test", &inputs).unwrap();
    b2.add(make_step("a", vec![], counter.clone(), 0)).unwrap();

    let s1 = b1.build();
    let s2 = b2.build();
    assert_eq!(s1.workflow_id, s2.workflow_id);
    assert_eq!(s1.steps[0].id, s2.steps[0].id);
}

#[tokio::test]
async fn parallel_independent_steps() {
    let db = fresh_db().await;
    let counter = Arc::new(AtomicUsize::new(0));

    // 8 independent steps; with cpu_concurrency=4 they run in two waves of 4.
    let mut b = WorkflowBuilder::new("test", &serde_json::json!({"case": "parallel"})).unwrap();
    for i in 0..8 {
        b.add(make_step(&format!("s{}", i), vec![], counter.clone(), 0))
            .unwrap();
    }

    let outcome = drive(
        db.clone(),
        b.build(),
        SchedulerPolicy {
            cpu_concurrency: 4,
            network_concurrency: 4,
            fail_fast: true,
            max_attempts: Some(3),
        },
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.status, TerminalStatus::Succeeded);
    assert_eq!(counter.load(Ordering::SeqCst), 8);
}

#[tokio::test]
async fn upstream_outputs_are_visible_to_dependents() {
    use std::sync::Mutex;

    let db = fresh_db().await;

    // Capture observed upstream outputs via a side channel.
    #[derive(Default)]
    struct Captured(Mutex<Vec<(String, serde_json::Value)>>);

    let captured: Arc<Captured> = Arc::new(Captured::default());

    struct Producer;
    #[derive(Serialize, Deserialize)]
    struct PIn {
        name: String,
    }
    #[derive(Serialize, Deserialize)]
    struct POut {
        artifact: String,
    }
    impl Step for Producer {
        type Inputs = PIn;
        type Outputs = POut;
        const KIND: &'static str = "test_producer";
        const RESOURCE_CLASS: ResourceClass = ResourceClass::Trivial;
        fn inputs(&self) -> &Self::Inputs {
            // Static inputs to make the test deterministic.
            static IN: std::sync::OnceLock<PIn> = std::sync::OnceLock::new();
            IN.get_or_init(|| PIn {
                name: "producer".into(),
            })
        }
        fn execute(
            self: Arc<Self>,
            _ctx: StepContext,
        ) -> BoxFuture<'static, Result<Self::Outputs>> {
            Box::pin(async move {
                Ok(POut {
                    artifact: "blob".into(),
                })
            })
        }
    }

    struct Consumer {
        deps: Vec<StepId>,
        captured: Arc<Captured>,
    }
    #[derive(Serialize, Deserialize)]
    struct CIn {
        marker: String,
    }
    impl Step for Consumer {
        type Inputs = CIn;
        type Outputs = serde_json::Value;
        const KIND: &'static str = "test_consumer";
        const RESOURCE_CLASS: ResourceClass = ResourceClass::Trivial;
        fn inputs(&self) -> &Self::Inputs {
            static IN: std::sync::OnceLock<CIn> = std::sync::OnceLock::new();
            IN.get_or_init(|| CIn { marker: "c".into() })
        }
        fn depends_on(&self) -> &[StepId] {
            &self.deps
        }
        fn execute(self: Arc<Self>, ctx: StepContext) -> BoxFuture<'static, Result<Self::Outputs>> {
            let captured = self.captured.clone();
            Box::pin(async move {
                let mut g = captured.0.lock().unwrap();
                for (k, v) in &ctx.upstream_outputs {
                    g.push((k.as_str().to_string(), v.clone()));
                }
                Ok(serde_json::Value::Null)
            })
        }
    }

    let mut b = WorkflowBuilder::new("test", &serde_json::json!({"case": "upstream"})).unwrap();
    let p = b.add(Producer).unwrap();
    b.add(Consumer {
        deps: vec![p.clone()],
        captured: captured.clone(),
    })
    .unwrap();

    drive(
        db,
        b.build(),
        policy(),
        std::env::temp_dir(),
        never_cancelled(),
    )
    .await
    .unwrap();

    let g = captured.0.lock().unwrap();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].0, p.as_str());
    assert_eq!(g[0].1["artifact"], "blob");
}
