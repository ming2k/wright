use std::path::{Path, PathBuf};

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::part::archive;
use wright::plan::manifest::{FabricateConfig, PlanManifest};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_manifest_without_dockyard(name: &str) -> (PlanManifest, PathBuf) {
    let manifest_path = fixture_path(name).join("plan.toml");
    let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
    for stage in manifest.lifecycle.values_mut() {
        stage.dockyard = "none".to_string();
    }
    (manifest, manifest_path.parent().unwrap().to_path_buf())
}

#[test]
fn test_build_hello_fixture() {
    let (manifest, plan_dir) = load_manifest_without_dockyard("hello");

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
            false,
            None,
            None,
            None,
        )
        .unwrap();

    // Verify the binary was built
    assert!(result.pkg_dir.join("usr/bin/hello").exists());
}

#[test]
fn test_build_and_archive_hello() {
    let (manifest, plan_dir) = load_manifest_without_dockyard("hello");

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
            false,
            None,
            None,
            None,
        )
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path =
        archive::create_archive(&result.pkg_dir, &manifest, output_dir.path()).unwrap();

    // Verify archive exists
    assert!(archive_path.exists());
    assert!(archive_path.to_string_lossy().ends_with(".wright.tar.zst"));

    // Verify we can read PARTINFO from it
    let pkginfo = archive::read_partinfo(&archive_path).unwrap();
    assert_eq!(pkginfo.name, "hello");
    assert_eq!(pkginfo.version, "1.0.0");
    assert_eq!(pkginfo.release, 1);
    assert_eq!(pkginfo.arch, "x86_64");

    // Verify we can extract it
    let extract_dir = tempfile::tempdir().unwrap();
    let extracted_info = archive::extract_archive(&archive_path, extract_dir.path()).unwrap();
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
dockyard = "none"
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
            false,
            None,
            None,
            None,
        )
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path =
        archive::create_archive(&result.pkg_dir, &manifest, output_dir.path()).unwrap();
    let pkginfo = archive::read_partinfo(&archive_path).unwrap();

    assert_eq!(pkginfo.runtime_deps, vec!["openssl", "zlib"]);

    let extract_dir = tempfile::tempdir().unwrap();
    archive::extract_archive(&archive_path, extract_dir.path()).unwrap();
    let partinfo = std::fs::read_to_string(extract_dir.path().join(".PARTINFO")).unwrap();
    assert!(partinfo.contains("runtime = [\"openssl\", \"zlib\"]"));
    assert!(!partinfo.contains("link ="));
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
    let (manifest, plan_dir) = load_manifest_without_dockyard("hello");

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
            false,
            None,
            None,
            None,
        )
        .unwrap();

    // Running only prepare: hello.c should exist but hello binary should not
    // (pkg_dir is recreated fresh for single-stage runs)
    assert!(result.src_dir.join("hello.c").exists());
    assert!(!result.pkg_dir.join("usr/bin/hello").exists());
}
