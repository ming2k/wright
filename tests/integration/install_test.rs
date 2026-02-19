use std::path::PathBuf;

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::database::Database;
use wright::package::archive;
use wright::package::manifest::PackageManifest;
use wright::transaction;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn build_hello_archive() -> PathBuf {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PackageManifest::from_file(&manifest_path).unwrap();
    let hold_dir = manifest_path.parent().unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder.build(&manifest, hold_dir, None, None, &std::collections::HashMap::new(), false, false).unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive =
        archive::create_archive(&result.pkg_dir, &manifest, output_dir.path()).unwrap();

    // Copy to persistent temp location
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let persistent = std::env::temp_dir().join(format!(
        "hello-integration-{}-{}.wright.tar.zst",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::copy(&archive, &persistent).unwrap();
    persistent
}

#[test]
fn test_end_to_end_install_query_remove() {
    let db = Database::open_in_memory().unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive();

    // Install
    transaction::install_package(&db, &archive, root.path(), false).unwrap();

    // Verify file exists on disk
    let hello_bin = root.path().join("usr/bin/hello");
    assert!(hello_bin.exists());

    // Query package in DB
    let pkg = db.get_package("hello").unwrap().unwrap();
    assert_eq!(pkg.name, "hello");
    assert_eq!(pkg.version, "1.0.0");
    assert_eq!(pkg.release, 1);
    assert_eq!(pkg.arch, "x86_64");

    // List packages
    let packages = db.list_packages().unwrap();
    assert_eq!(packages.len(), 1);

    // Query files
    let files = db.get_files(pkg.id).unwrap();
    assert!(files.iter().any(|f| f.path == "/usr/bin/hello"));

    // Find owner
    let owner = db.find_owner("/usr/bin/hello").unwrap();
    assert_eq!(owner, Some("hello".to_string()));

    // Verify integrity
    let issues = transaction::verify_package(&db, "hello", root.path()).unwrap();
    assert!(issues.is_empty());

    // Remove
    transaction::remove_package(&db, "hello", root.path(), false).unwrap();

    // Verify file is gone
    assert!(!hello_bin.exists());

    // Verify DB is clean
    assert!(db.get_package("hello").unwrap().is_none());
    assert!(db.list_packages().unwrap().is_empty());
    assert!(db.find_owner("/usr/bin/hello").unwrap().is_none());

    let _ = std::fs::remove_file(&archive);
}

#[test]
fn test_file_conflict_detection() {
    let db = Database::open_in_memory().unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive();

    // Install first copy
    transaction::install_package(&db, &archive, root.path(), false).unwrap();

    // Try to install again — should fail because package is already installed
    let result = transaction::install_package(&db, &archive, root.path(), false);
    assert!(result.is_err());

    let _ = std::fs::remove_file(&archive);
}

#[test]
fn test_verify_detects_modification() {
    let db = Database::open_in_memory().unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive();

    transaction::install_package(&db, &archive, root.path(), false).unwrap();

    // Tamper with installed file
    std::fs::write(root.path().join("usr/bin/hello"), b"tampered content").unwrap();

    let issues = transaction::verify_package(&db, "hello", root.path()).unwrap();
    assert!(!issues.is_empty());
    assert!(issues.iter().any(|i| i.contains("MODIFIED")));

    let _ = std::fs::remove_file(&archive);
}

#[test]
fn test_verify_detects_missing_file() {
    let db = Database::open_in_memory().unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive();

    transaction::install_package(&db, &archive, root.path(), false).unwrap();

    // Delete installed file
    std::fs::remove_file(root.path().join("usr/bin/hello")).unwrap();

    let issues = transaction::verify_package(&db, "hello", root.path()).unwrap();
    assert!(!issues.is_empty());
    assert!(issues.iter().any(|i| i.contains("MISSING")));

    let _ = std::fs::remove_file(&archive);
}

#[test]
fn test_search_installed_packages() {
    let db = Database::open_in_memory().unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive();

    transaction::install_package(&db, &archive, root.path(), false).unwrap();

    // Search by name
    let results = db.search_packages("hello").unwrap();
    assert_eq!(results.len(), 1);

    // Search by description
    let results = db.search_packages("World").unwrap();
    assert_eq!(results.len(), 1);

    // Search with no results
    let results = db.search_packages("nonexistent").unwrap();
    assert!(results.is_empty());

    let _ = std::fs::remove_file(&archive);
}
