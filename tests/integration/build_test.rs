use std::path::PathBuf;

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::package::archive;
use wright::package::manifest::PackageManifest;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn test_build_hello_fixture() {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PackageManifest::from_file(&manifest_path).unwrap();
    let hold_dir = manifest_path.parent().unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder.build(&manifest, hold_dir, &[], false, &std::collections::HashMap::new(), false, false, None).unwrap();

    // Verify the binary was built
    assert!(result.pkg_dir.join("usr/bin/hello").exists());
}

#[test]
fn test_build_and_archive_hello() {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PackageManifest::from_file(&manifest_path).unwrap();
    let hold_dir = manifest_path.parent().unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder.build(&manifest, hold_dir, &[], false, &std::collections::HashMap::new(), false, false, None).unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path =
        archive::create_archive(&result.pkg_dir, &manifest, output_dir.path()).unwrap();

    // Verify archive exists
    assert!(archive_path.exists());
    assert!(archive_path.to_string_lossy().ends_with(".wright.tar.zst"));

    // Verify we can read PKGINFO from it
    let pkginfo = archive::read_pkginfo(&archive_path).unwrap();
    assert_eq!(pkginfo.name, "hello");
    assert_eq!(pkginfo.version, "1.0.0");
    assert_eq!(pkginfo.release, 1);
    assert_eq!(pkginfo.arch, "x86_64");

    // Verify we can extract it
    let extract_dir = tempfile::tempdir().unwrap();
    let extracted_info = archive::extract_archive(&archive_path, extract_dir.path()).unwrap();
    assert_eq!(extracted_info.name, "hello");
    assert!(extract_dir.path().join("usr/bin/hello").exists());
    assert!(extract_dir.path().join(".PKGINFO").exists());
    assert!(extract_dir.path().join(".FILELIST").exists());
}

#[test]
fn test_lint_hello_fixture() {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PackageManifest::from_file(&manifest_path).unwrap();
    assert_eq!(manifest.plan.name, "hello");
    assert_eq!(manifest.plan.version, "1.0.0");
}

#[test]
fn test_lint_nginx_fixture() {
    let manifest_path = fixture_path("nginx").join("plan.toml");
    let manifest = PackageManifest::from_file(&manifest_path).unwrap();
    assert_eq!(manifest.plan.name, "nginx");
    assert_eq!(manifest.dependencies.runtime.len(), 3);
    assert!(manifest.install_scripts.is_some());
    assert!(manifest.backup.is_some());
}

#[test]
fn test_build_single_stage() {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PackageManifest::from_file(&manifest_path).unwrap();
    let hold_dir = manifest_path.parent().unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(&manifest, hold_dir, &["prepare".to_string()], false, &std::collections::HashMap::new(), false, false, None)
        .unwrap();

    // Running only prepare: hello.c should exist but hello binary should not
    assert!(result.src_dir.join("hello.c").exists());
    assert!(!result.pkg_dir.join("usr/bin/hello").exists());
}
