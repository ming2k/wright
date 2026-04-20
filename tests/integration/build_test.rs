use std::path::{Path, PathBuf};
use std::process::Command;

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::part::part;
use wright::plan::manifest::{FabricateConfig, PlanManifest};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_manifest_without_isolation(name: &str) -> (PlanManifest, PathBuf) {
    let manifest_path = fixture_path(name).join("plan.toml");
    let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
    for stage in manifest.lifecycle.values_mut() {
        stage.isolation = "none".to_string();
    }
    (manifest, manifest_path.parent().unwrap().to_path_buf())
}

#[test]
fn test_build_hello_fixture() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .unwrap();

    // Verify the binary was built
    assert!(result.output_dir.join("usr/bin/hello").exists());
}

#[test]
fn test_build_and_archive_hello() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path = part::create_part(&result.output_dir, &manifest, output_dir.path()).unwrap();

    // Verify archive exists
    assert!(archive_path.exists());
    assert!(archive_path.to_string_lossy().ends_with(".wright.tar.zst"));

    // Verify we can read PARTINFO from it
    let pkginfo = part::read_partinfo(&archive_path).unwrap();
    assert_eq!(pkginfo.name, "hello");
    assert_eq!(pkginfo.version, "1.0.0");
    assert_eq!(pkginfo.release, 1);
    assert_eq!(pkginfo.arch, "x86_64");

    // Verify we can extract it
    let extract_dir = tempfile::tempdir().unwrap();
    let (extracted_info, _hash) = part::extract_part(&archive_path, extract_dir.path()).unwrap();
    assert_eq!(extracted_info.name, "hello");
    assert!(extract_dir.path().join("usr/bin/hello").exists());
    assert!(extract_dir.path().join(".PARTINFO").exists());
    assert!(extract_dir.path().join(".FILELIST").exists());
}

#[test]
fn test_archive_records_runtime_but_not_link_dependencies() {
    let manifest = PlanManifest::parse(
        r#"
name = "runtime-link-overlap"
version = "1.0.0"
release = 1
description = "test part"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = ["openssl", "zlib"]
link = ["zlib", "libffi"]

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${PART_DIR}/usr/bin/runtime-link-overlap
"""
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let plan_dir = tempfile::tempdir().unwrap();
    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            plan_dir.path(),
            Path::new("/"),
            &[],
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path = part::create_part(&result.output_dir, &manifest, output_dir.path()).unwrap();
    let pkginfo = part::read_partinfo(&archive_path).unwrap();

    assert_eq!(pkginfo.runtime_deps, vec!["openssl", "zlib"]);

    let extract_dir = tempfile::tempdir().unwrap();
    part::extract_part(&archive_path, extract_dir.path()).unwrap();
    let partinfo = std::fs::read_to_string(extract_dir.path().join(".PARTINFO")).unwrap();
    assert!(partinfo.contains("runtime = [\"openssl\", \"zlib\"]"));
    assert!(!partinfo.contains("link ="));
}

#[test]
fn test_canonical_and_split_build_variables_are_available() {
    let manifest = PlanManifest::parse(
        r#"
name = "split-vars"
version = "1.0.0"
release = 1
description = "test canonical plan variables"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = []
build = []

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${PART_DIR}/usr/bin/${NAME}-${VERSION}
"""

[output."split-vars-doc"]
description = "doc output"
isolation = "none"
script = """
test -f ${MAIN_PART_DIR}/usr/bin/${MAIN_PART_NAME}-${VERSION}
install -Dm644 /dev/null ${PART_DIR}/usr/share/doc/${NAME}
"""
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let plan_dir = tempfile::tempdir().unwrap();
    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            plan_dir.path(),
            Path::new("/"),
            &[],
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .unwrap();

    assert!(result.output_dir.join("usr/bin/split-vars-1.0.0").exists());
    assert!(result.split_pkg_dirs["split-vars-doc"]
        .join("usr/share/doc/split-vars-doc")
        .exists());
}

#[test]
fn test_lint_hello_fixture() {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PlanManifest::from_file(&manifest_path).unwrap();
    assert_eq!(manifest.plan.name, "hello");
    assert_eq!(manifest.plan.version, "1.0.0");
}

#[test]
fn test_lint_nginx_fixture() {
    let manifest_path = fixture_path("nginx").join("plan.toml");
    let manifest = PlanManifest::from_file(&manifest_path).unwrap();
    assert_eq!(manifest.plan.name, "nginx");
    assert_eq!(manifest.dependencies.runtime.len(), 3);
    // Nginx uses fabricate metadata on the main output plus an extra doc output.
    match manifest.fabricate {
        Some(FabricateConfig::Multi(ref pkgs)) => {
            assert!(pkgs.contains_key("nginx"));
            assert!(pkgs.contains_key("nginx-doc"));
            let main = pkgs.get("nginx").unwrap();
            assert!(main.hooks.is_some());
            assert!(main.backup.is_some());
        }
        _ => panic!("expected Multi fabricate config for nginx"),
    }
}

#[test]
fn test_build_single_stage() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);

    // First do a full build so src/ directory exists
    builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .unwrap();

    // Now run a single stage on the existing build tree
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &["prepare".to_string()],
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .unwrap();

    // Running only prepare: hello.c should exist but hello binary should not
    // (output_dir is recreated fresh for single-stage runs)
    assert!(result.work_dir.join("hello.c").exists());
    assert!(!result.output_dir.join("usr/bin/hello").exists());
}

#[test]
fn test_print_parts_keeps_verbose_build_output_off_stdout() {
    let root = tempfile::tempdir().unwrap();
    let plans_dir = root.path().join("plans");
    let parts_dir = root.path().join("components");
    let cache_dir = root.path().join("cache");
    let db_dir = root.path().join("state");
    let logs_dir = root.path().join("logs");
    let build_dir = root.path().join("build");
    std::fs::create_dir_all(&plans_dir).unwrap();
    std::fs::create_dir_all(&parts_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::create_dir_all(&build_dir).unwrap();

    let plan_dir = plans_dir.join("verbose-pipe-test");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("plan.toml"),
        r#"
name = "verbose-pipe-test"
version = "1.0.0"
release = 1
description = "verify stdout/stderr split for --print-parts"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = []
build = []

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
echo LIVE-BUILD-OUTPUT
install -Dm755 /bin/sh ${PART_DIR}/usr/bin/verbose-pipe-test
"""
"#,
    )
    .unwrap();

    let config_path = root.path().join("wright.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
cache_dir = "{}"
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
            plans_dir.display(),
            parts_dir.display(),
            cache_dir.display(),
            db_dir.join("installed.db").display(),
            db_dir.join("archives.db").display(),
            logs_dir.display(),
            root.path().join("assemblies").display(),
            build_dir.display(),
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("-v")
        .arg("build")
        .arg("verbose-pipe-test")
        .arg("--print-parts")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wright build failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout_lines: Vec<_> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    assert_eq!(stdout_lines.len(), 1, "unexpected stdout: {stdout:?}");
    assert!(
        stdout_lines[0].ends_with(".wright.tar.zst"),
        "stdout should contain only archive paths: {stdout:?}"
    );
    assert!(
        !stdout.contains("LIVE-BUILD-OUTPUT"),
        "subprocess stdout leaked into stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("LIVE-BUILD-OUTPUT"),
        "expected live verbose build output on stderr: {stderr:?}"
    );
}
