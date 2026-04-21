use std::path::{Path, PathBuf};

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::database::InstalledDb;
use wright::part::part;
use wright::plan::manifest::PlanManifest;
use wright::transaction;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

async fn build_hello_archive() -> PathBuf {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
    for stage in manifest.lifecycle.values_mut() {
        stage.isolation = "none".to_string();
    }
    let plan_dir = manifest_path.parent().unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            plan_dir,
            Path::new("/"),
            &[],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive = part::create_part(&result.output_dir, &manifest, output_dir.path()).unwrap();

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

#[tokio::test]
async fn test_end_to_end_install_query_remove() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    // Install
    transaction::install_part(&db, &archive, root.path(), false)
        .await
        .unwrap();

    // Verify file exists on disk
    let hello_bin = root.path().join("usr/bin/hello");
    assert!(hello_bin.exists());

    // Query part in DB
    let part = db.get_part("hello").await.unwrap().unwrap();
    assert_eq!(part.name, "hello");
    assert_eq!(part.version, "1.0.0");
    assert_eq!(part.release, 1);
    assert_eq!(part.arch, "x86_64");

    // List parts
    let parts = db.list_parts().await.unwrap();
    assert_eq!(parts.len(), 1);

    // Query files
    let files = db.get_files(part.id).await.unwrap();
    assert!(files.iter().any(|f| f.path == "/usr/bin/hello"));

    // Find owner
    let owner = db.find_owner("/usr/bin/hello").await.unwrap();
    assert_eq!(owner, Some("hello".to_string()));

    // Verify integrity
    let issues = transaction::verify_part(&db, "hello", root.path())
        .await
        .unwrap();
    assert!(issues.is_empty());

    // Remove
    transaction::remove_part(&db, "hello", root.path(), false)
        .await
        .unwrap();

    // Verify file is gone
    assert!(!hello_bin.exists());

    // Verify DB is clean
    assert!(db.get_part("hello").await.unwrap().is_none());
    assert!(db.list_parts().await.unwrap().is_empty());
    assert!(db.find_owner("/usr/bin/hello").await.unwrap().is_none());

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_file_conflict_detection() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    // Install first copy
    transaction::install_part(&db, &archive, root.path(), false)
        .await
        .unwrap();

    // Try to install again — should fail because the part is already installed
    let result = transaction::install_part(&db, &archive, root.path(), false);
    assert!(result.await.is_err());

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_verify_detects_modification() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    transaction::install_part(&db, &archive, root.path(), false)
        .await
        .unwrap();

    // Tamper with installed file
    std::fs::write(root.path().join("usr/bin/hello"), b"tampered content").unwrap();

    let issues = transaction::verify_part(&db, "hello", root.path())
        .await
        .unwrap();
    assert!(!issues.is_empty());
    assert!(issues.iter().any(|i: &String| i.contains("MODIFIED")));

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_verify_detects_missing_file() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    transaction::install_part(&db, &archive, root.path(), false)
        .await
        .unwrap();

    // Delete installed file
    std::fs::remove_file(root.path().join("usr/bin/hello")).unwrap();

    let issues = transaction::verify_part(&db, "hello", root.path())
        .await
        .unwrap();
    assert!(!issues.is_empty());
    assert!(issues.iter().any(|i: &String| i.contains("MISSING")));

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_search_installed_parts() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    transaction::install_part(&db, &archive, root.path(), false)
        .await
        .unwrap();

    // Search by name
    let results = db.search_parts("hello").await.unwrap();
    assert_eq!(results.len(), 1);

    // Search by description
    let results = db.search_parts("World").await.unwrap();
    assert_eq!(results.len(), 1);

    // Search with no results
    let results = db.search_parts("nonexistent").await.unwrap();
    assert!(results.is_empty());

    let _ = std::fs::remove_file(&archive);
}
