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
        build_root_mode,
        0o755,
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
        staging_mode,
        0o755,
        "staging_dir ({}) must be 0755 despite strict umask, got {:o}",
        result.staging_dir.display(),
        staging_mode
    );

    // INV-PERM-01: Logs directory must be world-traversable (0755)
    let logs_dir = result.build_root.join("logs");
    assert!(logs_dir.exists(), "logs directory must exist");
    let logs_dir_mode = std::fs::metadata(&logs_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        logs_dir_mode,
        0o755,
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
        compile_log_mode,
        0o644,
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
        staging_log_mode,
        0o644,
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
    let outputs_vec = vec![(
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
    )];
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
        scratch_mode,
        0o700,
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

    let _guard =
        wright::util::logging::init_logging(&log_dir, tracing_subscriber::EnvFilter::new("debug"));

    unsafe { libc::umask(old_umask) };

    let dir_mode = std::fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        dir_mode,
        0o755,
        "log_dir ({}) must be 0755, got {:o}",
        log_dir.display(),
        dir_mode
    );

    let today_path = wright::util::logging::today_log_path(&log_dir);
    assert!(today_path.exists(), "today log file must exist");
    let file_mode = std::fs::metadata(&today_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        file_mode,
        0o644,
        "today_path ({}) must be 0644, got {:o}",
        today_path.display(),
        file_mode
    );
}

#[test]
fn test_sandbox_direct_exec_resets_umask() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut config = wright::sandbox::IsolationConfig::new(
        wright::sandbox::IsolationLevel::None,
        src.path().to_path_buf(),
        out.path().to_path_buf(),
        "perm-test-task".to_string(),
    );

    let old_umask = unsafe { libc::umask(0o077) };
    let output = wright::sandbox::run_in_isolation(
        &mut config,
        "/bin/sh",
        &["-c".to_string(), "umask".to_string()],
    )
    .unwrap();
    unsafe { libc::umask(old_umask) };

    assert!(output.status.success());
    let stdout = output.stdout.tail.trim().to_string();
    assert_eq!(stdout, "0022", "child process umask must be reset to 0022");
}

#[test]
fn test_staging_and_archive_sealing_permissions_under_strict_umask() {
    let old_umask = unsafe { libc::umask(0o077) };

    let staging_tmp = tempfile::tempdir().unwrap();
    let staging_dir = staging_tmp.path();

    // Create nested directories with restricted 0700 mode (simulating sudo umask 0077)
    let nested_dir = staging_dir.join("usr/libexec/mytool/31.1");
    std::fs::create_dir_all(&nested_dir).unwrap();
    std::fs::set_permissions(&nested_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    let share_dir = staging_dir.join("usr/share/mytool/lisp");
    std::fs::create_dir_all(&share_dir).unwrap();
    std::fs::set_permissions(&share_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

    // Create executable with 0700 mode
    let helper_bin = nested_dir.join("helper");
    std::fs::write(&helper_bin, b"#!/bin/sh\necho ok\n").unwrap();
    std::fs::set_permissions(&helper_bin, std::fs::Permissions::from_mode(0o700)).unwrap();

    // Create data file with 0600 mode
    let lisp_file = share_dir.join("init.el");
    std::fs::write(&lisp_file, b";; lisp init\n").unwrap();
    std::fs::set_permissions(&lisp_file, std::fs::Permissions::from_mode(0o600)).unwrap();

    // Create archive
    let out_tmp = tempfile::tempdir().unwrap();
    let archive_path = out_tmp.path().join("test.tar.zst");
    wright::part::compression::create_tar_zst(staging_dir, &archive_path).unwrap();

    // Verify tar archive headers contain canonicalized permissions (0755 dirs/executables, 0644 files)
    let file = std::fs::File::open(&archive_path).unwrap();
    let decoder = zstd::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        let mode = entry.header().mode().unwrap() & 0o777;
        let entry_type = entry.header().entry_type();
        if entry_type == tar::EntryType::Directory {
            assert_eq!(
                mode, 0o755,
                "tar directory entry '{path}' must be 0755, got {mode:o}"
            );
        } else if path.ends_with("helper") {
            assert_eq!(
                mode, 0o755,
                "tar executable entry '{path}' must be 0755, got {mode:o}"
            );
        } else if path.ends_with("init.el") {
            assert_eq!(
                mode, 0o644,
                "tar data file entry '{path}' must be 0644, got {mode:o}"
            );
        }
    }

    // Verify unpacking under strict umask 0077 still materializes 0755 dirs and 0644 files
    let extract_tmp = tempfile::tempdir().unwrap();
    wright::part::compression::extract_tar_zst(&archive_path, extract_tmp.path()).unwrap();

    unsafe { libc::umask(old_umask) };

    let extracted_nested = extract_tmp.path().join("usr/libexec/mytool/31.1");
    let extracted_share = extract_tmp.path().join("usr/share/mytool/lisp");
    let extracted_helper = extracted_nested.join("helper");
    let extracted_lisp = extracted_share.join("init.el");

    let nested_mode =
        std::fs::metadata(&extracted_nested).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        nested_mode, 0o755,
        "unpacked directory must be 0755, got {nested_mode:o}"
    );

    let share_mode =
        std::fs::metadata(&extracted_share).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        share_mode, 0o755,
        "unpacked directory must be 0755, got {share_mode:o}"
    );

    let helper_mode =
        std::fs::metadata(&extracted_helper).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        helper_mode, 0o755,
        "unpacked executable must be 0755, got {helper_mode:o}"
    );

    let lisp_mode =
        std::fs::metadata(&extracted_lisp).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        lisp_mode, 0o644,
        "unpacked data file must be 0644, got {lisp_mode:o}"
    );
}

#[tokio::test]
async fn test_deploy_directory_and_file_materialization_under_strict_umask() {
    let old_umask = unsafe { libc::umask(0o077) };

    let db = wright::database::InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    // Pre-create an existing directory with restricted 0700 permissions on the target root
    // to test that deployment heals existing restricted directories!
    let preexisting_dir = root.path().join("usr/share/mytool");
    std::fs::create_dir_all(&preexisting_dir).unwrap();
    std::fs::set_permissions(&preexisting_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        std::fs::metadata(&preexisting_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );

    // Build a test package
    let plan_tmp = tempfile::tempdir().unwrap();
    let plan_toml = r#"
name = "pkg-perm-test"
version = "1.0.0"
release = 1
arch = "x86_64"
description = "permission test part"
license = "MIT"

[pipeline.staging]
isolation = "none"
script = """
mkdir -p ${STAGING_DIR}/usr/libexec/mytool/31.1
mkdir -p ${STAGING_DIR}/usr/share/mytool/lisp
echo '#!/bin/sh' > ${STAGING_DIR}/usr/libexec/mytool/31.1/helper
chmod 700 ${STAGING_DIR}/usr/libexec/mytool/31.1/helper
echo '; lisp' > ${STAGING_DIR}/usr/share/mytool/lisp/init.el
"""
"#;
    std::fs::write(plan_tmp.path().join("plan.toml"), plan_toml).unwrap();
    let manifest = PlanManifest::from_file(&plan_tmp.path().join("plan.toml")).unwrap();
    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.forge_dir = build_tmp.path().to_path_buf();
    let parts_tmp = tempfile::tempdir().unwrap();
    config.general.parts_dir = parts_tmp.path().to_path_buf();

    let foundry = Foundry::new(config.clone());
    let result = foundry
        .build(
            &manifest,
            plan_tmp.path(),
            Path::new("/"),
            BuildOptions::default(),
        )
        .await
        .unwrap();

    let archive = wright::part::archive::create_part(&result.staging_dir, &manifest, parts_tmp.path(), None).unwrap();
    assert!(archive.exists(), "sealed archive must exist at {}", archive.display());

    let session = wright::database::SessionContext {
        id: "perm-test-session".into(),
        command: "install".into(),
    };

    // Deploy to target root under strict umask 0077
    wright::transaction::deploy_part(&db, &archive, root.path(), false, session, ledger.path())
        .await
        .unwrap();

    unsafe { libc::umask(old_umask) };

    // Verify all target directories are 0755
    let usr_dir = root.path().join("usr");
    let libexec_dir = root.path().join("usr/libexec");
    let mytool_libexec = root.path().join("usr/libexec/mytool");
    let mytool_ver = root.path().join("usr/libexec/mytool/31.1");
    let share_dir = root.path().join("usr/share");
    let share_mytool = root.path().join("usr/share/mytool");
    let share_lisp = root.path().join("usr/share/mytool/lisp");

    for dir in [
        &usr_dir,
        &libexec_dir,
        &mytool_libexec,
        &mytool_ver,
        &share_dir,
        &share_mytool,
        &share_lisp,
    ] {
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "target directory '{}' must be 0755, got {mode:o}",
            dir.display()
        );
    }

    // Verify files on target root
    let helper_file = mytool_ver.join("helper");
    let init_el_file = share_lisp.join("init.el");

    let helper_mode = std::fs::metadata(&helper_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        helper_mode, 0o755,
        "executable file '{}' must be 0755, got {helper_mode:o}",
        helper_file.display()
    );

    let init_el_mode = std::fs::metadata(&init_el_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        init_el_mode, 0o644,
        "data file '{}' must be 0644, got {init_el_mode:o}",
        init_el_file.display()
    );
}
