//! Output-channel and SIGPIPE policy (see `wright_engine::util::output`):
//!
//! * A closed stdout reader (`wright list | head`) is a quiet exit 0 — never
//!   a panic (101) or a SIGPIPE death (141).
//! * Build stages exec user code with the traditional default signal
//!   dispositions restored, so shell pipelines relying on SIGPIPE death
//!   behave normally inside builds.

use std::process::{Command, Stdio};

/// Write a minimal isolated config and return its path.
fn isolated_config(root: &tempfile::TempDir) -> std::path::PathBuf {
    let plans_dir = root.path().join("plans");
    let parts_dir = root.path().join("parts");
    let source_dir = root.path().join("sources");
    let db_dir = root.path().join("state");
    let logs_dir = root.path().join("logs");
    let forge_dir = root.path().join("build");
    for dir in [
        &plans_dir,
        &parts_dir,
        &source_dir,
        &db_dir,
        &logs_dir,
        &forge_dir,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

    let config_path = root.path().join("wright.toml");
    std::fs::write(
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
            plans_dir.display(),
            parts_dir.display(),
            source_dir.display(),
            db_dir.join("wright.db").display(),
            logs_dir.display(),
            root.path().join("assemblies").display(),
            forge_dir.display(),
        ),
    )
    .unwrap();
    config_path
}

#[test]
fn closed_stdout_pipe_exits_quietly_with_zero() {
    let root = tempfile::tempdir().unwrap();
    let config_path = isolated_config(&root);

    let mut child = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("list")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The reader goes away before wright can print: the first stdout write
    // hits a fully closed pipe.
    drop(child.stdout.take());

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "a closed stdout pipe must exit quietly with code 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("panicked"),
        "no panic may escape through stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn partial_stdout_read_exits_quietly_with_zero() {
    let root = tempfile::tempdir().unwrap();
    let config_path = isolated_config(&root);

    let mut child = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("list")
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // `| head` semantics: consume a fixed prefix, then close.
    use std::io::Read;
    let mut stdout = child.stdout.take().unwrap();
    let mut prefix = [0u8; 1];
    let _ = stdout.read(&mut prefix);
    drop(stdout);

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "a partial stdout read must exit quietly with code 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Build stages run user shell code, which is written against traditional
/// Unix signal semantics: SIGPIPE must be back at its default disposition
/// by the time the stage's shell execs (wright itself keeps it ignored).
///
/// The mask is recorded with `grep` — deliberately not `awk` (gawk ignores
/// SIGPIPE in its own startup, which would measure the tool instead of the
/// stage) and not command substitution (a subshell indirection).
#[test]
fn build_stage_runs_with_default_sigpipe_disposition() {
    let root = tempfile::tempdir().unwrap();
    let config_path = isolated_config(&root);

    let plans_dir = root.path().join("plans");
    let plan_dir = plans_dir.join("sigpipe-env-check");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("plan.toml"),
        r#"
name = "sigpipe-env-check"
version = "1.0.0"
release = 1
description = "verify build stages see default SIGPIPE disposition"
license = "MIT"
arch = "x86_64"

link_deps = []

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
mkdir -p "${STAGING_DIR}/usr/bin"
: > "${STAGING_DIR}/usr/bin/sigpipe-env-check"
grep '^SigIgn' /proc/self/status > "${STAGING_DIR}/sigign.txt"
"""
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("build")
        .arg("sigpipe-env-check")
        .arg("--until-stage")
        .arg("staging")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wright build failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // SIGPIPE is signal 13 → bit 12 (0x1000) in the ignored-signals mask.
    let status_line = std::fs::read_to_string(
        root.path()
            .join("build/sigpipe-env-check-1.0.0/staging/sigign.txt"),
    )
    .unwrap();
    let mask_hex = status_line
        .split_whitespace()
        .nth(1)
        .expect("SigIgn line carries a hex mask");
    let mask = u64::from_str_radix(mask_hex, 16).unwrap();
    assert_eq!(
        mask & 0x1000,
        0,
        "SIGPIPE must not be inherited as ignored inside a build stage (SigIgn={mask:#x})"
    );
}
