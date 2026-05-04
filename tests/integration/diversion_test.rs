use std::collections::HashMap;
use std::path::{Path, PathBuf};

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::database::InstalledDb;
use wright::part::part;
use wright::plan::manifest::PlanManifest;
use wright::transaction;

async fn create_test_archive(name: &str, shared_file_content: &str) -> PathBuf {
    let tmp = tempfile::tempdir().unwrap();
    let plan_dir = tmp.path().to_path_buf();

    let plan_toml = format!(
        r#"
name = "{}"
version = "1.0.0"
release = 1
arch = "x86_64"
description = "test part"
license = "MIT"

[lifecycle.prepare]
isolation = "none"
script = "mkdir -p usr/bin"

[lifecycle.compile]
isolation = "none"
script = "mkdir -p ${{STAGING_DIR}}/usr/bin && echo -n '{}' > ${{STAGING_DIR}}/usr/bin/shared"
"#,
        name, shared_file_content
    );

    std::fs::write(plan_dir.join("plan.toml"), plan_toml).unwrap();

    let manifest = PlanManifest::from_file(&plan_dir.join("plan.toml")).unwrap();
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
            None,
            false,
            false,
            &HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive = part::create_part(&result.output_dir, &manifest, output_dir.path()).unwrap();

    let persistent = std::env::temp_dir().join(format!(
        "test-diversion-{}-{}.wright.tar.zst",
        name,
        std::process::id()
    ));
    std::fs::copy(&archive, &persistent).unwrap();
    persistent
}

#[tokio::test]
async fn test_file_diversion_and_restoration() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();

    let archive_a = create_test_archive("part-a", "content-a").await;
    let archive_b = create_test_archive("part-b", "content-b").await;

    // 1. Install Part A
    transaction::install_part(&db, &archive_a, root.path(), false)
        .await
        .unwrap();
    let shared_path = root.path().join("usr/bin/shared");
    assert_eq!(std::fs::read_to_string(&shared_path).unwrap(), "content-a");

    // 2. Install Part B (conflicts with A)
    transaction::install_part(&db, &archive_b, root.path(), false)
        .await
        .unwrap();

    // 3. Verify diversion
    assert_eq!(std::fs::read_to_string(&shared_path).unwrap(), "content-b");
    let diverted_path = root.path().join("usr/bin/shared.wright-diverted");
    assert!(diverted_path.exists());
    assert_eq!(
        std::fs::read_to_string(&diverted_path).unwrap(),
        "content-a"
    );

    // Check DB
    let b_part = db.get_part("part-b").await.unwrap().unwrap();
    let diverted = db
        .get_diverted_file("/usr/bin/shared", b_part.id)
        .await
        .unwrap();
    assert_eq!(
        diverted,
        Some("/usr/bin/shared.wright-diverted".to_string())
    );

    // 4. Remove Part B
    transaction::remove_part(&db, "part-b", root.path(), false)
        .await
        .unwrap();

    // 5. Verify restoration
    assert_eq!(std::fs::read_to_string(&shared_path).unwrap(), "content-a");
    assert!(!diverted_path.exists());

    let _ = std::fs::remove_file(&archive_a);
    let _ = std::fs::remove_file(&archive_b);
}
