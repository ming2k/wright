//! Batch failure semantics, end to end through the real CLI.
//!
//! Tasks within one batch have no inter-dependencies, so a failing task
//! must not interrupt its siblings: the failure is announced immediately,
//! the batch settles once no task is left running, and the failed batch
//! blocks the next one.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

struct BatchWorkspace {
    _temp: tempfile::TempDir,
    root: PathBuf,
    config_path: PathBuf,
    plans: PathBuf,
    logs: PathBuf,
}

impl BatchWorkspace {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let plans = temp.path().join("plans");
        let parts = temp.path().join("parts");
        let sources = temp.path().join("sources");
        let state = temp.path().join("wright");
        let logs = temp.path().join("logs");
        let build = temp.path().join("build");
        for dir in [&root, &plans, &parts, &sources, &state, &logs, &build] {
            fs::create_dir_all(dir).unwrap();
        }

        let config_path = temp.path().join("wright.toml");
        fs::write(
            &config_path,
            format!(
                r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
source_dir = "{}"
db_path = "{}"
logs_dir = "{}"
executors_dir = "/etc/wright/executors"
assemblies_dir = "{}"

[build]
forge_dir = "{}"
default_isolation = "none"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
                plans.display(),
                parts.display(),
                sources.display(),
                state.join("wright.db").display(),
                logs.display(),
                temp.path().join("assemblies").display(),
                build.display(),
            ),
        )
        .unwrap();

        Self {
            _temp: temp,
            root,
            config_path,
            plans,
            logs,
        }
    }

    fn write_plan(&self, name: &str, extra_header: &str, staging_script: &str) {
        let dir = self.plans.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("plan.toml"),
            format!(
                r#"
name = "{name}"
version = "1.0.0"
release = 1
description = "{name}"
license = "MIT"
arch = "x86_64"
{extra_header}
[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
{staging_script}
"""
"#,
            ),
        )
        .unwrap();
    }

    fn wright(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_wright"))
            .arg("--config")
            .arg(&self.config_path)
            .args(args)
            .output()
            .unwrap()
    }
}

/// A task that fails fast must neither kill its slower sibling nor let the
/// next batch start: the sibling runs to completion, the failure is
/// announced immediately and again in the terminal report, and nothing
/// from the failed batch (or any later one) is deployed.
#[test]
fn batch_failure_does_not_interrupt_siblings_and_blocks_next_batch() {
    let ws = BatchWorkspace::new();
    let marker = ws.plans.join("sibling-completed");

    ws.write_plan(
        "batch-slow",
        "",
        &format!(
            "sleep 1\ninstall -Dm644 /dev/null ${{STAGING_DIR}}/usr/share/batch-slow\ntouch {}",
            marker.display()
        ),
    );
    ws.write_plan("batch-fail", "", "exit 3");
    ws.write_plan(
        "batch-next",
        "link_deps = []",
        "install -Dm644 /dev/null ${STAGING_DIR}/usr/share/batch-next",
    );
    // batch-next depends on batch-slow, pushing it into the second batch.
    let next_dir = ws.plans.join("batch-next");
    let mut plan = fs::read_to_string(next_dir.join("plan.toml")).unwrap();
    plan.push_str(
        r#"
[[output]]
name = "batch-next"
runtime_deps = ["batch-slow"]
"#,
    );
    fs::write(next_dir.join("plan.toml"), plan).unwrap();

    let started = Instant::now();
    let output = ws.wright(&[
        "install",
        "--root",
        ws.root.to_str().unwrap(),
        "batch-next",
        "batch-fail",
        "--deps",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "a failed batch must fail the command; stderr: {stderr}"
    );
    assert!(
        marker.exists(),
        "slower sibling must run to completion despite the failure; stderr: {stderr}"
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(1),
        "command returned before the sibling finished; stderr: {stderr}"
    );

    // Immediate notice at failure time plus the terminal report headline.
    let notices = stderr.matches("task 'batch-fail' failed").count();
    assert!(
        notices >= 2,
        "expected an immediate notice and the terminal report, got {notices}; stderr: {stderr}"
    );
    let first_notice = stderr.find("task 'batch-fail' failed").unwrap();
    let settlement = stderr.find("for the full trace").unwrap_or(usize::MAX);
    assert!(
        first_notice < settlement,
        "immediate notice must precede the terminal report; stderr: {stderr}"
    );
    // The stage-log pointer stays on the same line as its cause.
    assert!(
        stderr.contains("failed with exit code 3 (see log:"),
        "see-log path must not be split off its cause; stderr: {stderr}"
    );

    // The failed batch is rolled back and the next batch never starts.
    assert!(
        !ws.root.join("usr/share/batch-slow").exists(),
        "failed batch must not be deployed; stderr: {stderr}"
    );
    assert!(
        !ws.root.join("usr/share/batch-next").exists(),
        "next batch must not start after a failed batch; stderr: {stderr}"
    );
}

/// A task that fails as the only (last-running) task of its batch is
/// reported exactly once: no mid-run notice duplicates the terminal
/// report, and the terminal output cleanly ends at the log hint without
/// dumping verbose timing tables.
#[test]
fn single_failure_reports_once_and_terminal_is_clean() {
    let ws = BatchWorkspace::new();
    ws.write_plan("solo-fail", "", "exit 3");

    let output = ws.wright(&["install", "--root", ws.root.to_str().unwrap(), "solo-fail"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert_eq!(
        stderr.matches("task 'solo-fail' failed").count(),
        1,
        "a failure that empties its batch must be reported exactly once; stderr: {stderr}"
    );
    assert!(
        stderr.contains("for the full trace"),
        "failure report must end with log hint; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("install step timing:"),
        "terminal failure output must not dump timing tables; stderr: {stderr}"
    );

    // Verify structured timing telemetry was persisted to the daily log file.
    let log_files: Vec<_> = fs::read_dir(&ws.logs)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert!(!log_files.is_empty(), "a log file must be created");
    let log_content = fs::read_to_string(&log_files[0]).unwrap();
    assert!(
        log_content.contains("workflow.timing"),
        "log file must contain structured timing event; log: {log_content}"
    );
}

/// The direct `build` path (drive.rs) follows the same rule: a failure
/// that empties its batch appears in the terminal report only.
#[test]
fn build_single_failure_reports_once() {
    let ws = BatchWorkspace::new();
    ws.write_plan("solo-build-fail", "", "exit 3");

    let output = ws.wright(&["build", "solo-build-fail"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert_eq!(
        stderr.matches("task 'solo-build-fail' failed").count(),
        1,
        "a failure that empties its batch must be reported exactly once; stderr: {stderr}"
    );
}

/// Two failures in one batch settle into a single aggregated report that
/// lists every failed task.
#[test]
fn multiple_batch_failures_are_reported_together() {
    let ws = BatchWorkspace::new();
    ws.write_plan("agg-fail-a", "", "exit 3");
    ws.write_plan("agg-fail-b", "", "exit 5");

    let output = ws.wright(&["build", "agg-fail-a", "agg-fail-b"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "a failed batch must fail the command; stderr: {stderr}"
    );
    assert!(
        stderr.contains("2 tasks failed"),
        "expected the aggregated settlement headline; stderr: {stderr}"
    );
    assert!(
        stderr.contains("Caused by:"),
        "expected the cause list; stderr: {stderr}"
    );
    for task in ["agg-fail-a", "agg-fail-b"] {
        assert!(
            stderr.contains(&format!("task '{task}' failed")),
            "aggregated report must list {task}; stderr: {stderr}"
        );
    }
}
