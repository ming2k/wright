use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use wright::dockyard::{run_in_dockyard, DockyardConfig, DockyardLevel};
use wright::error::WrightError;

fn should_skip_dockyard_test(err: &WrightError) -> bool {
    let msg = err.to_string();
    msg.contains("Namespace isolation unavailable")
        || msg.contains("dockyard level none")
        || msg.contains("unshare:")
        || msg.contains("Operation not permitted")
        || msg.contains("Permission denied")
}

fn run_shebang_script_from_src(src: &Path) -> Result<(String, String), WrightError> {
    let pkg = tempfile::tempdir().unwrap();
    let script = src.join("hello.sh");
    std::fs::write(&script, "#!/bin/sh\necho dockyard-ok\n").unwrap();
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();

    let config = DockyardConfig::new(
        DockyardLevel::Strict,
        src.to_path_buf(),
        pkg.path().to_path_buf(),
        "dockyard-shebang-repro".to_string(),
    );

    let args = vec!["-lc".to_string(), "./hello.sh".to_string()];
    let output = run_in_dockyard(&config, "/bin/bash", &args)?;
    let stdout = output.stdout.tail.trim().to_string();
    let stderr = output.stderr.tail.trim().to_string();
    assert!(
        output.status.success(),
        "expected shebang exec to succeed, status={:?}, stdout={stdout:?}, stderr={stderr:?}",
        output.status.code()
    );
    Ok((stdout, stderr))
}

#[test]
fn dockyard_executes_shebang_script_from_build_mount() {
    let src = tempfile::tempdir().unwrap();
    let (stdout, stderr) = match run_shebang_script_from_src(src.path()) {
        Ok(output) => output,
        Err(err) if should_skip_dockyard_test(&err) => return,
        Err(err) => panic!("dockyard run failed unexpectedly: {err}"),
    };
    assert_eq!(stdout, "dockyard-ok");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
}

#[test]
fn dockyard_executes_shebang_script_from_var_tmp_build_mount() {
    let root = match tempfile::tempdir_in("/var/tmp") {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let src = root.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let (stdout, stderr) = match run_shebang_script_from_src(&src) {
        Ok(output) => output,
        Err(err) if should_skip_dockyard_test(&err) => return,
        Err(err) => panic!("dockyard run failed unexpectedly: {err}"),
    };
    assert_eq!(stdout, "dockyard-ok");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr:?}");
}
