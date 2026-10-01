use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use wright::config::GlobalConfig;
use wright::foundry::{BuildOptions, Foundry};
use wright::plan::manifest::PlanManifest;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_manifest_without_isolation(name: &str) -> (PlanManifest, PathBuf) {
    let manifest_path = fixture_path(name).join("plan.toml");
    let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
    for stage in manifest.pipeline.values_mut() {
        stage.isolation = Some("none".to_string());
    }
    (manifest, manifest_path.parent().unwrap().to_path_buf())
}

#[tokio::test]
async fn test_build_log_and_workspace_permissions_under_strict_umask() {
    // Save current umask and set to strict 0077 (simulating sudo Defaults umask=0077)
    let old_umask = unsafe { libc::umask(0o077) };

    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.forge_dir = build_tmp.path().to_path_buf();

    let foundry = Foundry::new(config);
    let result = foundry
        .build(
            &manifest,
            plan_dir.as_ref(),
            Path::new("/"),
            BuildOptions::default(),
        )
        .await
        .unwrap();

    // Restore umask
    unsafe { libc::umask(old_umask) };

    // INV-PERM-01: Build root and staging dir must be world-traversable (0755)
    let build_root_mode = std::fs::metadata(&result.build_root)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        build_root_mode, 0o755,
        "build_root ({}) must be 0755 despite strict umask, got {:o}",
        result.build_root.display(),
        build_root_mode
    );

    let staging_mode = std::fs::metadata(&result.staging_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        staging_mode, 0o755,
        "staging_dir ({}) must be 0755 despite strict umask, got {:o}",
        result.staging_dir.display(),
        staging_mode
    );

    // INV-PERM-01: Logs directory must be world-traversable (0755)
    let logs_dir = result.build_root.join("logs");
    assert!(logs_dir.exists(), "logs directory must exist");
    let logs_dir_mode = std::fs::metadata(&logs_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        logs_dir_mode, 0o755,
        "logs_dir ({}) must be 0755, got {:o}",
        logs_dir.display(),
        logs_dir_mode
    );

    // INV-PERM-01: Compile and staging log files must be world-readable (0644)
    let compile_log = logs_dir.join("compile.log");
    assert!(compile_log.exists(), "compile.log must exist");
    let compile_log_mode = std::fs::metadata(&compile_log)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        compile_log_mode, 0o644,
        "compile.log ({}) must be 0644, got {:o}",
        compile_log.display(),
        compile_log_mode
    );
    let compile_content = std::fs::read_to_string(&compile_log).unwrap();
    assert!(compile_content.contains("Stage: compile"));

    let staging_log = logs_dir.join("staging.log");
    assert!(staging_log.exists(), "staging.log must exist");
    let staging_log_mode = std::fs::metadata(&staging_log)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        staging_log_mode, 0o644,
        "staging.log ({}) must be 0644, got {:o}",
        staging_log.display(),
        staging_log_mode
    );
}

#[tokio::test]
async fn test_slice_errors_log_world_readable() {
    let old_umask = unsafe { libc::umask(0o077) };

    let (mut manifest, plan_dir) = load_manifest_without_isolation("hello");

    // Configure multi-output with partial rule so that unclaimed staging files exist
    let mut outputs_vec = Vec::new();
    outputs_vec.push((
        "sub".to_string(),
        wright::plan::manifest::SubFabricateOutput {
            include: Some(vec!["nonexistent/**".to_string()]),
            description: None,
            version: None,
            release: None,
            arch: None,
            license: None,
            runtime_deps: Vec::new(),
            replaces: Vec::new(),
            conflicts: Vec::new(),
            exclude: None,
            hooks: None,
            backup: None,
        },
    ));
    manifest.outputs = Some(wright::plan::manifest::OutputConfig::Multi(outputs_vec));

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.forge_dir = build_tmp.path().to_path_buf();

    let foundry = Foundry::new(config);
    let build_err = foundry
        .build(
            &manifest,
            plan_dir.as_ref(),
            Path::new("/"),
            BuildOptions::default(),
        )
        .await
        .unwrap_err();

    unsafe { libc::umask(old_umask) };

    let err_msg = build_err.to_string();
    assert!(
        err_msg.contains("slice-errors.log"),
        "error must point to slice-errors.log: {err_msg}"
    );

    let build_root = build_tmp.path().join(format!(
        "{}-{}",
        manifest.metadata.name,
        manifest.metadata.version.as_deref().unwrap()
    ));
    let slice_errors_log = build_root.join("logs/slice-errors.log");
    assert!(slice_errors_log.exists(), "slice-errors.log must exist");

    let log_mode = std::fs::metadata(&slice_errors_log)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        log_mode, 0o644,
        "slice-errors.log must be world-readable (0644), got {:o}",
        log_mode
    );
}

#[test]
fn test_isolation_scratch_retains_private_mode() {
    let old_umask = unsafe { libc::umask(0o077) };

    let tmp = tempfile::tempdir().unwrap();
    let scratch_parent = tmp.path().join(".wright-isolation");
    let _scratch = scratch_parent.join("task-1");

    // Use wright-sandbox native scratch logic
    let _ = std::fs::create_dir_all(&scratch_parent);
    std::fs::set_permissions(&scratch_parent, std::fs::Permissions::from_mode(0o700)).unwrap();

    unsafe { libc::umask(old_umask) };

    // INV-PERM-02: Scratch directory must remain strictly 0700
    let scratch_mode = std::fs::metadata(&scratch_parent)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        scratch_mode, 0o700,
        "isolation scratch ({}) must remain 0700, got {:o}",
        scratch_parent.display(),
        scratch_mode
    );
}

#[test]
fn test_daily_log_permissions_under_strict_umask() {
    let old_umask = unsafe { libc::umask(0o077) };

    let tmp = tempfile::tempdir().unwrap();
    let log_dir = tmp.path().join("logs");

    let _guard = wright::util::logging::init_logging(
        &log_dir,
        tracing_subscriber::EnvFilter::new("debug"),
    );

    unsafe { libc::umask(old_umask) };

    let dir_mode = std::fs::metadata(&log_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        dir_mode, 0o755,
        "log_dir ({}) must be 0755, got {:o}",
        log_dir.display(),
        dir_mode
    );

    let today_path = wright::util::logging::today_log_path(&log_dir);
    assert!(today_path.exists(), "today log file must exist");
    let file_mode = std::fs::metadata(&today_path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        file_mode, 0o644,
        "today_path ({}) must be 0644, got {:o}",
        today_path.display(),
        file_mode
    );
}
