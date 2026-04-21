use std::fs;
use std::path::PathBuf;
use std::process::Command;
use wright::cli::resolve::{DomainArg, MatchPolicyArg};
use wright::cli::system::Commands as SystemCommands;
use wright::commands::system;
use wright::config::GlobalConfig;

#[tokio::test]
async fn test_apply_with_dependency_in_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans = temp.path().join("plans");
    let parts = temp.path().join("parts");
    let state = temp.path().join("state");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&plans).unwrap();
    fs::create_dir_all(&parts).unwrap();
    fs::create_dir_all(&state).unwrap();

    let db_path = state.join("installed.db");
    let archive_db_path = state.join("archives.db");

    // Create a dependency plan 'wayland' in CWD (actually in the temp dir where we'll run)
    let wayland_dir = plans.join("wayland");
    fs::create_dir_all(&wayland_dir).unwrap();
    fs::write(
        wayland_dir.join("plan.toml"),
        r#"
name = "wayland"
version = "1.22.0"
release = 1
description = "Wayland"
license = "MIT"
arch = "x86_64"
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "mkdir -p ${PART_DIR}/usr/lib"
"#,
    )
    .unwrap();

    // Create a target plan 'wayland-utils' that depends on 'wayland'
    let utils_dir = plans.join("wayland-utils");
    fs::create_dir_all(&utils_dir).unwrap();
    fs::write(
        utils_dir.join("plan.toml"),
        r#"
name = "wayland-utils"
version = "1.2.0"
release = 1
description = "Wayland utils"
license = "MIT"
arch = "x86_64"
[dependencies]
runtime = ["wayland"]
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "mkdir -p ${PART_DIR}/usr/bin"
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    config.general.parts_dir = parts;
    config.general.installed_db_path = db_path.clone();
    config.general.archive_db_path = archive_db_path;
    config.general.plans_dir = PathBuf::from("/nonexistent"); // Don't use default

    // Change CWD to the plans directory
    let old_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&plans).unwrap();

    let cmd = SystemCommands::Apply {
        targets: vec!["wayland-utils".to_string()],
        resume: None,
        deps: Some(DomainArg::All),
        rdeps: None,
        match_policies: vec![MatchPolicyArg::Missing],
        depth: Some(0),
        force: false,
        dry_run: true,
    };

    let result = system::execute(cmd, &config, &db_path, &root, 2, false).await;

    std::env::set_current_dir(old_cwd).unwrap();

    assert!(result.is_ok(), "Apply failed: {:?}", result.err());
}

#[test]
fn test_apply_resume_continues_after_partial_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans = temp.path().join("plans");
    let parts = temp.path().join("parts");
    let sources = temp.path().join("sources");
    let state = temp.path().join("state");
    let logs = temp.path().join("logs");
    let build = temp.path().join("build");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&plans).unwrap();
    fs::create_dir_all(&parts).unwrap();
    fs::create_dir_all(&sources).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&build).unwrap();

    let signal_path = temp.path().join("allow-main");

    let dep_dir = plans.join("apply-resume-dep");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::write(
        dep_dir.join("plan.toml"),
        r#"
name = "apply-resume-dep"
version = "1.0.0"
release = 1
description = "dep"
license = "MIT"
arch = "x86_64"
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "install -Dm644 /dev/null ${PART_DIR}/usr/share/apply-resume-dep"
"#,
    )
    .unwrap();

    let main_dir = plans.join("apply-resume-main");
    fs::create_dir_all(&main_dir).unwrap();
    fs::write(
        main_dir.join("plan.toml"),
        format!(
            r#"
name = "apply-resume-main"
version = "1.0.0"
release = 1
description = "main"
license = "MIT"
arch = "x86_64"
[dependencies]
runtime = ["apply-resume-dep"]
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
test -f "{}"
install -Dm644 /dev/null ${{PART_DIR}}/usr/share/apply-resume-main
"""
"#,
            signal_path.display()
        ),
    )
    .unwrap();

    let config_path = temp.path().join("wright.toml");
    fs::write(
        &config_path,
        format!(
            r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
source_dir = "{}"
installed_db_path = "{}"
archive_db_path = "{}"
logs_dir = "{}"
executors_dir = "/etc/wright/executors"
assemblies_dir = "{}"

[build]
build_dir = "{}"
default_isolation = "none"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
            plans.display(),
            parts.display(),
            sources.display(),
            state.join("installed.db").display(),
            state.join("archives.db").display(),
            logs.display(),
            temp.path().join("assemblies").display(),
            build.display(),
        ),
    )
    .unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("--root")
        .arg(&root)
        .arg("apply")
        .arg("apply-resume-main")
        .arg("--deps")
        .output()
        .unwrap();

    assert!(
        !first.status.success(),
        "first apply should fail to leave a resumable session"
    );
    assert!(
        root.join("usr/share/apply-resume-dep").exists(),
        "dependency batch should have been applied before failure"
    );
    assert!(
        !root.join("usr/share/apply-resume-main").exists(),
        "main target should not be installed before resume"
    );

    fs::write(&signal_path, "ok").unwrap();

    let second = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("--root")
        .arg(&root)
        .arg("apply")
        .arg("apply-resume-main")
        .arg("--deps")
        .arg("--resume")
        .output()
        .unwrap();

    assert!(
        second.status.success(),
        "resume apply failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(root.join("usr/share/apply-resume-main").exists());
}
