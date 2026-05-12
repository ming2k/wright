use std::fs;
use std::path::PathBuf;
use std::process::Command;
use wright::cli::common::{DomainArg, MatchPolicyArg};
use wright::cli::system::InstallArgs;
use wright::commands::system;
use wright::config::GlobalConfig;

#[tokio::test]
async fn test_install_with_dependency_in_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans = temp.path().join("plans");
    let parts = temp.path().join("parts");
    let state = temp.path().join("wright");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&plans).unwrap();
    fs::create_dir_all(&parts).unwrap();
    fs::create_dir_all(&state).unwrap();

    let db_path = state.join("wright.db");

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
script = "mkdir -p ${STAGING_DIR}/usr/lib"
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
link_deps = []
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "mkdir -p ${STAGING_DIR}/usr/bin"

[[output]]
name = "wayland-utils"
runtime_deps = ["wayland"]
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    config.general.parts_dir = parts;
    config.general.db_path = db_path.clone();
    config.general.plans_dir = PathBuf::from("/nonexistent"); // Don't use default

    // Change CWD to the plans directory
    let old_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&plans).unwrap();

    let cmd = InstallArgs {
        targets: vec!["wayland-utils".to_string()],
        deps: Some(DomainArg::All),
        rdeps: None,
        match_policies: vec![MatchPolicyArg::Missing],
        depth: Some(0),
        force: false,
        dry_run: true,
        root: None,
    };

    let result = system::dispatch_install(cmd, &config, &db_path, &root, 2, false).await;

    std::env::set_current_dir(old_cwd).unwrap();

    assert!(result.is_ok(), "Install failed: {:?}", result.err());
}

#[test]
fn test_install_resume_continues_after_partial_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans = temp.path().join("plans");
    let parts = temp.path().join("parts");
    let sources = temp.path().join("sources");
    let state = temp.path().join("wright");
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

    let dep_dir = plans.join("install-resume-dep");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::write(
        dep_dir.join("plan.toml"),
        r#"
name = "install-resume-dep"
version = "1.0.0"
release = 1
description = "dep"
license = "MIT"
arch = "x86_64"
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "install -Dm644 /dev/null ${STAGING_DIR}/usr/share/install-resume-dep"
"#,
    )
    .unwrap();

    let main_dir = plans.join("install-resume-main");
    fs::create_dir_all(&main_dir).unwrap();
    fs::write(
        main_dir.join("plan.toml"),
        format!(
            r#"
name = "install-resume-main"
version = "1.0.0"
release = 1
description = "main"
license = "MIT"
arch = "x86_64"
link_deps = []
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
test -f "{}"
install -Dm644 /dev/null ${{STAGING_DIR}}/usr/share/install-resume-main
"""

[[output]]
name = "install-resume-main"
runtime_deps = ["install-resume-dep"]
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

    let first = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("--root")
        .arg(&root)
        .arg("install")
        .arg("install-resume-main")
        .arg("--deps")
        .output()
        .unwrap();

    assert!(
        !first.status.success(),
        "first install should fail to leave a resumable session"
    );
    assert!(
        root.join("usr/share/install-resume-dep").exists(),
        "dependency batch should have been installed before failure"
    );
    assert!(
        !root.join("usr/share/install-resume-main").exists(),
        "main target should not be installed before resume"
    );

    fs::write(&signal_path, "ok").unwrap();

    // Rerunning the same command auto-resumes — no --resume flag needed
    // under the workflow model.
    let second = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("--root")
        .arg(&root)
        .arg("install")
        .arg("install-resume-main")
        .arg("--deps")
        .output()
        .unwrap();

    assert!(
        second.status.success(),
        "resume install failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(root.join("usr/share/install-resume-main").exists());
}

#[tokio::test]
async fn test_install_resolve_build_set_includes_runtime_deps() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans = temp.path().join("plans");
    let parts = temp.path().join("parts");
    let state = temp.path().join("wright");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&plans).unwrap();
    fs::create_dir_all(&parts).unwrap();
    fs::create_dir_all(&state).unwrap();

    let db_path = state.join("wright.db");

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
script = "mkdir -p ${STAGING_DIR}/usr/lib"
"#,
    )
    .unwrap();

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
link_deps = []
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "mkdir -p ${STAGING_DIR}/usr/bin"

[[output]]
name = "wayland-utils"
runtime_deps = ["wayland"]
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    config.general.parts_dir = parts;
    config.general.db_path = db_path.clone();
    config.general.plans_dir = plans.clone();

    let opts = wright::resolve::ResolveOptions {
        deps: wright::resolve::DepDomain::ALL,
        rdeps: wright::resolve::DepDomain::empty(),
        match_policies: vec![wright::resolve::MatchPolicy::Missing],
        depth: Some(0),
        include_targets: true,
        preserve_targets: false,
    };

    let build_set =
        wright::resolve::resolve_build_set(&config, vec!["wayland-utils".to_string()], opts)
            .await
            .unwrap();

    println!("Build set: {:?}", build_set);
    assert!(
        build_set.contains(&"wayland".to_string()),
        "wayland (runtime dep) should be in build set, got: {:?}",
        build_set
    );
    assert!(
        build_set.contains(&"wayland-utils".to_string()),
        "wayland-utils should be in build set"
    );
}

#[test]
fn test_install_installs_runtime_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans = temp.path().join("plans");
    let parts = temp.path().join("parts");
    let sources = temp.path().join("sources");
    let state = temp.path().join("wright");
    let logs = temp.path().join("logs");
    let build = temp.path().join("build");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&plans).unwrap();
    fs::create_dir_all(&parts).unwrap();
    fs::create_dir_all(&sources).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&build).unwrap();

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
script = "install -Dm644 /dev/null ${STAGING_DIR}/usr/share/wayland-installed"
"#,
    )
    .unwrap();

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
link_deps = []
[lifecycle.staging]
executor = "shell"
isolation = "none"
script = "install -Dm644 /dev/null ${STAGING_DIR}/usr/share/wayland-utils-installed"

[[output]]
name = "wayland-utils"
runtime_deps = ["wayland"]
"#,
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

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("--root")
        .arg(&root)
        .arg("install")
        .arg("wayland-utils")
        .arg("--deps")
        .output()
        .unwrap();

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));

    assert!(
        output.status.success(),
        "install failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        root.join("usr/share/wayland-installed").exists(),
        "runtime dependency 'wayland' should be installed"
    );
    assert!(
        root.join("usr/share/wayland-utils-installed").exists(),
        "target 'wayland-utils' should be installed"
    );
}
