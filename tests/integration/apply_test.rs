use std::fs;
use std::path::PathBuf;
use wright::config::GlobalConfig;
use wright::commands::system;
use wright::cli::system::Commands as SystemCommands;
use wright::cli::resolve::{DomainArg, MatchPolicyArg};

#[test]
fn test_apply_with_dependency_in_cwd() {
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
    fs::write(wayland_dir.join("plan.toml"), r#"
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
"#).unwrap();

    // Create a target plan 'wayland-utils' that depends on 'wayland'
    let utils_dir = plans.join("wayland-utils");
    fs::create_dir_all(&utils_dir).unwrap();
    fs::write(utils_dir.join("plan.toml"), r#"
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
"#).unwrap();

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
        deps: Some(DomainArg::All),
        rdeps: None,
        match_policies: vec![MatchPolicyArg::Missing],
        depth: Some(0),
        force: false,
        dry_run: true,
    };

    let result = system::execute(
        cmd,
        &config,
        &db_path,
        &root,
        2,
        false,
    );

    std::env::set_current_dir(old_cwd).unwrap();

    assert!(result.is_ok(), "Apply failed: {:?}", result.err());
}
