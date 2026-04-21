use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use wright::error::WrightError;
use wright::isolation::{run_in_isolation, IsolationConfig, IsolationLevel};

fn should_skip_isolation_test(err: &WrightError) -> bool {
    let msg = err.to_string();
    msg.contains("Namespace isolation unavailable")
        || msg.contains("isolation level none")
        || msg.contains("unshare:")
        || msg.contains("Operation not permitted")
        || msg.contains("Permission denied")
}

fn run_shebang_script_from_src(src: &Path) -> Result<(String, String), WrightError> {
    let part = tempfile::tempdir().unwrap();
    let script = src.join("hello.sh");
    std::fs::write(&script, "#!/bin/sh\necho isolation-ok\n").unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let mut config = IsolationConfig::new(
        IsolationLevel::Strict,
        src.to_path_buf(),
        part.path().to_path_buf(),
        "isolation-shebang-repro".to_string(),
    );

    let args = vec!["-lc".to_string(), "./hello.sh".to_string()];
    let output = run_in_isolation(&mut config, "/bin/bash", &args)?;
    let stdout = output.stdout.tail.trim().to_string();
    let stderr = output.stderr.tail.trim().to_string();
    assert!(
        output.status.success(),
        "expected shebang exec to succeed, status={:?}, stdout={stdout:?}, stderr={stderr:?}",
        output.status.code()
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
