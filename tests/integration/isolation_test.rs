use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use wright::error::WrightError;
use wright::isolation::{IsolationConfig, IsolationLevel, run_in_isolation};

fn run_direct_command(
    src: &Path,
    body: &str,
    timeout_secs: Option<u64>,
) -> Result<(Option<i32>, String, String), WrightError> {
    let output_dir = tempfile::tempdir().unwrap();
    let mut config = IsolationConfig::new(
        IsolationLevel::None,
        src.to_path_buf(),
        output_dir.path().to_path_buf(),
        "direct-execution".to_string(),
    );
    config.rlimits.timeout_secs = timeout_secs;
    let output = run_in_isolation(
        &mut config,
        "/bin/sh",
        &["-c".to_string(), body.to_string()],
    )?;
    Ok((
        output.status.code(),
        output.stdout.tail.trim().to_string(),
        output.stderr.tail.trim().to_string(),
    ))
}

fn should_skip_isolation_test(err: &WrightError) -> bool {
    let msg = err.to_string();
    msg.contains("Namespace isolation unavailable")
        || msg.contains("isolation level none")
        || msg.contains("unshare:")
        || msg.contains("Operation not permitted")
        || msg.contains("Permission denied")
}

fn run_script_from_src(
    src: &Path,
    body: &str,
    timeout_secs: Option<u64>,
) -> Result<(Option<i32>, String, String), WrightError> {
    let part = tempfile::tempdir().unwrap();
    let script = src.join("hello.sh");
    std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let mut config = IsolationConfig::new(
        IsolationLevel::Strict,
        src.to_path_buf(),
        part.path().to_path_buf(),
        "isolation-shebang-repro".to_string(),
    );
    config.helper_executable = Some(Path::new(env!("CARGO_BIN_EXE_wright")).to_path_buf());
    config.rlimits.timeout_secs = timeout_secs;

    let args = vec!["-lc".to_string(), "./hello.sh".to_string()];
    let output = run_in_isolation(&mut config, "/bin/bash", &args)?;
    let stdout = output.stdout.tail.trim().to_string();
    let stderr = output.stderr.tail.trim().to_string();
    Ok((output.status.code(), stdout, stderr))
}

fn run_shebang_script_from_src(src: &Path) -> Result<(String, String), WrightError> {
    let (status, stdout, stderr) = run_script_from_src(src, "echo isolation-ok", None)?;
    assert_eq!(
        status,
        Some(0),
        "expected shebang exec to succeed, stdout={stdout:?}, stderr={stderr:?}"
    );
    Ok((stdout, stderr))
}

#[tokio::test]
async fn isolation_executes_shebang_script_from_build_mount() {
    let src = tempfile::tempdir().unwrap();
    let (stdout, stderr) = match run_shebang_script_from_src(src.path()) {
        Ok(output) => output,
        Err(err) if should_skip_isolation_test(&err) => return,
        Err(err) => panic!("isolation run failed unexpectedly: {err}"),
    };
    assert_eq!(stdout, "isolation-ok");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
}

#[tokio::test]
async fn isolation_executes_shebang_script_from_var_tmp_build_mount() {
    let root = match tempfile::tempdir_in("/var/tmp") {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let src = root.path().join("work");
    std::fs::create_dir_all(&src).unwrap();
    let (stdout, stderr) = match run_shebang_script_from_src(&src) {
        Ok(output) => output,
        Err(err) if should_skip_isolation_test(&err) => return,
        Err(err) => panic!("isolation run failed unexpectedly: {err}"),
    };
    assert_eq!(stdout, "isolation-ok");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
}

#[tokio::test]
async fn isolation_helper_preserves_output_streams_and_exit_code() {
    let src = tempfile::tempdir().unwrap();
    let result = run_script_from_src(
        src.path(),
        "echo helper-stdout; echo helper-stderr >&2; exit 7",
        None,
    );
    let (status, stdout, stderr) = match result {
        Ok(output) => output,
        Err(err) if should_skip_isolation_test(&err) => return,
        Err(err) => panic!("isolation run failed unexpectedly: {err}"),
    };
    assert_eq!(status, Some(7));
    assert_eq!(stdout, "helper-stdout");
    assert_eq!(stderr, "helper-stderr");
}

#[tokio::test]
async fn isolation_helper_timeout_terminates_the_namespace() {
    let src = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let result = run_script_from_src(src.path(), "sleep 30", Some(1));
    let (status, _, _) = match result {
        Ok(output) => output,
        Err(err) if should_skip_isolation_test(&err) => return,
        Err(err) => panic!("isolation run failed unexpectedly: {err}"),
    };
    assert_ne!(status, Some(0));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "timeout did not terminate the helper promptly"
    );
}

#[tokio::test]
async fn direct_execution_preserves_output_streams_and_exit_code() {
    let src = tempfile::tempdir().unwrap();
    let (status, stdout, stderr) = run_direct_command(
        src.path(),
        "echo direct-stdout; echo direct-stderr >&2; exit 7",
        None,
    )
    .unwrap();

    assert_eq!(status, Some(7));
    assert_eq!(stdout, "direct-stdout");
    assert_eq!(stderr, "direct-stderr");
}

#[tokio::test]
async fn direct_timeout_terminates_the_process_group() {
    let src = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let (status, _, _) = run_direct_command(src.path(), "sleep 30 & wait", Some(1)).unwrap();

    assert_ne!(status, Some(0));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "timeout did not terminate the direct process group promptly"
    );
}
