use std::path::{Path, PathBuf};

use wright::cli::common::Context;
use wright::cli::install::{self, InstallArgs};
use wright::config::GlobalConfig;
use wright::database::InstalledDb;
use wright::database::SessionContext;
use wright::foundry::{BuildOptions, Foundry};
use wright::part::archive;
use wright::part::store::LocalPartStore;
use wright::plan::manifest::{OutputConfig, PlanManifest};
use wright::transaction;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

async fn build_hello_archive() -> PathBuf {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
    for stage in manifest.pipeline.values_mut() {
        stage.isolation = Some("none".to_string());
    }
    let plan_dir = manifest_path.parent().unwrap();

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

    let output_dir = tempfile::tempdir().unwrap();
    let archive =
        archive::create_part(&result.staging_dir, &manifest, output_dir.path(), None).unwrap();

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

fn persistent_archive_path(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "{}-integration-{}-{}.wright.tar.zst",
        name,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

fn build_split_archive(version: &str, output_name: &str) -> PathBuf {
    let manifest = split_manifest(version);
    create_split_archive_from_manifest(&manifest, output_name, None)
}

fn split_manifest(version: &str) -> PlanManifest {
    PlanManifest::parse(&format!(
        r#"
name = "split-plan"
version = "{version}"
release = 1
description = "split plan"
license = "MIT"
arch = "x86_64"

[[output]]
name = "x"
description = "x output"
include = ["/usr/bin/x"]

[[output]]
name = "y"
description = "y output"
include = ["/usr/bin/y"]

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${{STAGING_DIR}}/usr/bin/x
install -Dm755 /bin/sh ${{STAGING_DIR}}/usr/bin/y
"""
"#
    ))
    .unwrap()
}

fn create_split_archive_from_manifest(
    manifest: &PlanManifest,
    output_name: &str,
    output_dir: Option<&Path>,
) -> PathBuf {
    let OutputConfig::Multi(outputs) = manifest.outputs.as_ref().unwrap();
    let sub = outputs
        .iter()
        .find(|(name, _)| name == output_name)
        .map(|(_, sub)| sub)
        .unwrap();
    let sub_manifest = sub.to_manifest(output_name, manifest);

    let part_dir = tempfile::tempdir().unwrap();
    let bin_path = part_dir.path().join("usr/bin").join(output_name);
    std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
    std::fs::write(
        &bin_path,
        format!("#!/bin/sh\nprintf '{}\\n'\n", output_name),
    )
    .unwrap();

    let temp_output_dir = tempfile::tempdir().unwrap();
    let output_dir = output_dir.unwrap_or(temp_output_dir.path());
    let archive =
        archive::create_part(part_dir.path(), &sub_manifest, output_dir, Some(manifest)).unwrap();

    if output_dir == temp_output_dir.path() {
        let version = manifest.metadata.version.as_deref().unwrap_or("noversion");
        let persistent = persistent_archive_path(&format!("{}-{}", output_name, version));
        std::fs::copy(&archive, &persistent).unwrap();
        persistent
    } else {
        archive
    }
}

fn write_split_plan(plans_dir: &Path, version: &str) -> PathBuf {
    let plan_dir = plans_dir.join("split-plan");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("plan.toml"),
        format!(
            r#"
name = "split-plan"
version = "{version}"
release = 1
description = "split plan"
license = "MIT"
arch = "x86_64"

[[output]]
name = "x"
description = "x output"
include = ["/usr/bin/x"]

[[output]]
name = "y"
description = "y output"
include = ["/usr/bin/y"]

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${{STAGING_DIR}}/usr/bin/x
install -Dm755 /bin/sh ${{STAGING_DIR}}/usr/bin/y
"""
"#
        ),
    )
    .unwrap();
    plan_dir
}

#[tokio::test]
async fn test_end_to_end_install_query_remove() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    // Deploy
    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    // Verify file exists on disk
    let hello_bin = root.path().join("usr/bin/hello");
    assert!(hello_bin.exists());

    // Query part in DB via list_parts (part-centric interface)
    let parts = db.list_parts().await.unwrap();
    assert_eq!(parts.len(), 1);
    let part = &parts[0];
    assert_eq!(part.name, "hello");
    assert_eq!(part.version, "1.0.0");
    assert_eq!(part.release, 1);
    assert_eq!(part.arch, "x86_64");

    // Query files
    let files = db.get_files(part.id).await.unwrap();
    assert!(files.iter().any(|f| f.path == "/usr/bin/hello"));

    // Verify integrity
    let issues = transaction::verify_part(&db, "hello", root.path())
        .await
        .unwrap();
    assert!(issues.is_empty());

    // Remove
    transaction::remove_part(
        &db,
        "hello",
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
    )
    .await
    .unwrap();

    // Verify file is gone
    assert!(!hello_bin.exists());

    // Verify DB is clean
    assert!(db.get_part("hello").await.unwrap().is_none());
    assert!(db.list_parts().await.unwrap().is_empty());

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_install_accepts_same_revision_split_outputs() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let part_store = LocalPartStore::new();
    let x = build_split_archive("1.0.0", "x");
    let y = build_split_archive("1.0.0", "y");

    transaction::deploy_parts(
        &db,
        &[x.clone(), y.clone()],
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    let plan = db.get_plan("split-plan").await.unwrap().unwrap();
    assert_eq!(plan.version, "1.0.0");
    assert!(db.get_part("x").await.unwrap().is_some());
    assert!(db.get_part("y").await.unwrap().is_some());

    let _ = std::fs::remove_file(&x);
    let _ = std::fs::remove_file(&y);
}

#[tokio::test]
async fn test_install_command_resolves_plan_name_to_all_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let plans_dir = temp.path().join("plans");
    let parts_dir = temp.path().join("parts");
    let state_dir = temp.path().join("state");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&plans_dir).unwrap();
    std::fs::create_dir_all(&parts_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    write_split_plan(&plans_dir, "1.0.0");
    let manifest = split_manifest("1.0.0");
    let x_archive = create_split_archive_from_manifest(&manifest, "x", Some(&parts_dir));
    let y_archive = create_split_archive_from_manifest(&manifest, "y", Some(&parts_dir));
    assert_eq!(
        x_archive.file_name().and_then(|name| name.to_str()),
        Some("x-1.0.0-1-x86_64.wright.tar.zst")
    );
    assert_eq!(
        y_archive.file_name().and_then(|name| name.to_str()),
        Some("y-1.0.0-1-x86_64.wright.tar.zst")
    );

    let db_path = state_dir.join("wright.db");
    let mut config = GlobalConfig::default();
    config.general.plans_dir = plans_dir;
    config.general.parts_dir = parts_dir;
    config.general.db_path = db_path.clone();
    config.build.forge_dir = temp.path().join("build");

    let cmd = InstallArgs {
        targets: vec!["split-plan".to_string()],
        deps: None,
        rdeps: None,
        match_policies: vec![],
        deps_depth: None,
        rdeps_depth: None,
        force: false,
        fresh: false,
        dry_run: false,
        root: None,
        no_inhibit_rebuild: false,
    };
    let ctx = Context {
        config: &config,
        db_path: db_path.clone(),
        root_dir: root.clone(),
        verbose: 0,
        quiet: false,
    };
    install::run(cmd, &ctx).await.unwrap();

    assert!(root.join("usr/bin/x").exists());
    assert!(root.join("usr/bin/y").exists());

    let db = InstalledDb::open(&db_path, None).await.unwrap();
    assert!(db.get_part("x").await.unwrap().is_some());
    assert!(db.get_part("y").await.unwrap().is_some());
}

#[tokio::test]
async fn test_install_rejects_mixed_split_plan_revisions() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let part_store = LocalPartStore::new();
    let x = build_split_archive("1.0.0", "x");
    let y = build_split_archive("2.0.0", "y");

    let err = transaction::deploy_parts(
        &db,
        &[x.clone(), y.clone()],
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("mixed revisions"));
    assert!(db.list_parts().await.unwrap().is_empty());

    let _ = std::fs::remove_file(&x);
    let _ = std::fs::remove_file(&y);
}

#[tokio::test]
async fn test_install_rejects_revision_change_that_leaves_installed_outputs() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let part_store = LocalPartStore::new();
    let x_v1 = build_split_archive("1.0.0", "x");
    let y_v2 = build_split_archive("2.0.0", "y");

    transaction::deploy_parts(
        &db,
        std::slice::from_ref(&x_v1),
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    let err = transaction::deploy_parts(
        &db,
        std::slice::from_ref(&y_v2),
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("deployed output(s)"));
    assert!(err.to_string().contains("x"));
    assert!(db.get_part("x").await.unwrap().is_some());
    assert!(db.get_part("y").await.unwrap().is_none());

    let _ = std::fs::remove_file(&x_v1);
    let _ = std::fs::remove_file(&y_v2);
}

#[tokio::test]
async fn test_successful_install_removes_rollback_journal() {
    let state = tempfile::tempdir().unwrap();
    let db_path = state.path().join("wright.db");
    let journal_path = db_path.with_extension("journal");
    let db = InstalledDb::open(&db_path, None).await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    assert!(
        !journal_path.exists(),
        "successful deploy left rollback journal at {}",
        journal_path.display()
    );

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_file_conflict_detection() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    // Deploy first copy
    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    // Try to deploy again — should fail because the part is already installed
    let ledger = tempfile::tempdir().unwrap();
    let result = transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        ledger.path(),
    );
    assert!(result.await.is_err());

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_verify_detects_modification() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
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

    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
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
async fn test_list_installed_parts() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let archive = build_hello_archive().await;

    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        SessionContext {
            id: "test".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    // list_parts returns all installed parts with plan metadata
    let results = db.list_parts().await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "hello");

    // get_part returns a single part
    let part = db.get_part("hello").await.unwrap();
    assert!(part.is_some());
    assert_eq!(part.unwrap().name, "hello");

    // Nonexistent part
    let part = db.get_part("nonexistent").await.unwrap();
    assert!(part.is_none());

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_upgrade_retires_stale_outputs_and_cleans_files() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let part_store = LocalPartStore::new();

    let x_v1 = build_split_archive("1.0.0", "x");
    let y_v1 = build_split_archive("1.0.0", "y");

    // Initially deploy split-plan v1 with both x and y
    transaction::deploy_parts(
        &db,
        &[x_v1.clone(), y_v1.clone()],
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test-v1".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    assert!(db.get_part("x").await.unwrap().is_some());
    assert!(db.get_part("y").await.unwrap().is_some());
    assert!(root.path().join("usr/bin/x").exists());
    assert!(root.path().join("usr/bin/y").exists());

    // In v2, split-plan drops output y and only produces x
    let v2_source = r#"
name = "split-plan"
version = "2.0.0"
release = 1
description = "split plan v2 with single output"
license = "MIT"
arch = "x86_64"

[[output]]
name = "x"
description = "x output"
include = ["/usr/bin/x"]

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${STAGING_DIR}/usr/bin/x
"""
"#;
    let mut v2_manifest = PlanManifest::parse(v2_source).unwrap();
    v2_manifest.plan_source = Some(v2_source.to_string());

    let x_v2 = create_split_archive_from_manifest(&v2_manifest, "x", None);

    // Deploy v2 batch containing only x
    transaction::deploy_parts(
        &db,
        std::slice::from_ref(&x_v2),
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test-v2".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    // Verify x is upgraded to 2.0.0
    let x_part = db
        .get_part("x")
        .await
        .unwrap()
        .expect("x should be installed");
    let plan = db
        .get_plan_by_id(x_part.plan_id)
        .await
        .unwrap()
        .expect("plan exists");
    assert_eq!(plan.version, "2.0.0");
    assert!(root.path().join("usr/bin/x").exists());

    // Verify y was automatically retired: unregistered from db and its files deleted with zero leftovers
    assert!(db.get_part("y").await.unwrap().is_none());
    assert!(
        !root.path().join("usr/bin/y").exists(),
        "stale output file /usr/bin/y must be cleaned up"
    );

    let plan_parts = db.get_parts_by_plan("split-plan").await.unwrap();
    assert_eq!(plan_parts.len(), 1);
    assert_eq!(plan_parts[0].name, "x");

    let _ = std::fs::remove_file(&x_v1);
    let _ = std::fs::remove_file(&y_v1);
    let _ = std::fs::remove_file(&x_v2);
}

#[tokio::test]
async fn test_upgrade_with_replaces_cleans_replaced_part() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let part_store = LocalPartStore::new();

    let x_v1 = build_split_archive("1.0.0", "x");

    transaction::deploy_parts(
        &db,
        std::slice::from_ref(&x_v1),
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test-v1".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    assert!(db.get_part("x").await.unwrap().is_some());
    assert!(root.path().join("usr/bin/x").exists());

    // In v2, output is renamed to 'z', which declares replaces = ["x"]
    let v2_manifest = PlanManifest::parse(
        r#"
name = "split-plan"
version = "2.0.0"
release = 1
description = "split plan v2 replacing x"
license = "MIT"
arch = "x86_64"

[[output]]
name = "z"
description = "z replaces x"
replaces = ["x"]
include = ["/usr/bin/z"]

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${STAGING_DIR}/usr/bin/z
"""
"#,
    )
    .unwrap();

    let z_v2 = create_split_archive_from_manifest(&v2_manifest, "z", None);

    transaction::deploy_parts(
        &db,
        std::slice::from_ref(&z_v2),
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test-v2".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    assert!(db.get_part("z").await.unwrap().is_some());
    assert!(root.path().join("usr/bin/z").exists());

    // x is cleanly removed
    assert!(db.get_part("x").await.unwrap().is_none());
    assert!(!root.path().join("usr/bin/x").exists());

    let _ = std::fs::remove_file(&x_v1);
    let _ = std::fs::remove_file(&z_v2);
}

#[tokio::test]
async fn test_upgrade_stale_output_retirement_blocked_by_dependents_without_force() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let part_store = LocalPartStore::new();

    let x_v1 = build_split_archive("1.0.0", "x");
    let y_v1 = build_split_archive("1.0.0", "y");

    transaction::deploy_parts(
        &db,
        &[x_v1.clone(), y_v1.clone()],
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test-v1".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    let dep_m = PlanManifest::parse(
        r#"
name = "dep-app"
version = "1.0.0"
release = 1
description = "dependent app"
license = "MIT"
arch = "x86_64"

[[output]]
name = "dep-app"
description = "dep-app output"
runtime_deps = ["y"]
include = ["/usr/bin/dep-app"]

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${STAGING_DIR}/usr/bin/dep-app
"""
"#,
    )
    .unwrap();

    let dep_archive = create_split_archive_from_manifest(&dep_m, "dep-app", None);
    transaction::deploy_parts(
        &db,
        std::slice::from_ref(&dep_archive),
        root.path(),
        &part_store,
        false,
        false,
        true,
        SessionContext {
            id: "test-dep".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    let v2_source = r#"
name = "split-plan"
version = "2.0.0"
release = 1
description = "split plan v2 with single output"
license = "MIT"
arch = "x86_64"

[[output]]
name = "x"
description = "x output"
include = ["/usr/bin/x"]

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${STAGING_DIR}/usr/bin/x
"""
"#;
    let mut v2_manifest = PlanManifest::parse(v2_source).unwrap();
    v2_manifest.plan_source = Some(v2_source.to_string());
    let x_v2 = create_split_archive_from_manifest(&v2_manifest, "x", None);

    // Deploying without force should fail because 'dep-app' depends on 'y'
    let err = transaction::deploy_parts(
        &db,
        std::slice::from_ref(&x_v2),
        root.path(),
        &part_store,
        false, // force: false
        false,
        true,
        SessionContext {
            id: "test-v2".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("cannot retire stale output 'y'"));
    assert!(err.to_string().contains("dep-app"));

    // Deploying WITH force = true should succeed and retire 'y'
    transaction::deploy_parts(
        &db,
        std::slice::from_ref(&x_v2),
        root.path(),
        &part_store,
        true, // force: true
        false,
        true,
        SessionContext {
            id: "test-v2-forced".into(),
            command: "test".into(),
        },
        tempfile::tempdir().unwrap().path(),
    )
    .await
    .unwrap();

    assert!(db.get_part("y").await.unwrap().is_none());
    assert!(!root.path().join("usr/bin/y").exists());

    let _ = std::fs::remove_file(&x_v1);
    let _ = std::fs::remove_file(&y_v1);
    let _ = std::fs::remove_file(&dep_archive);
    let _ = std::fs::remove_file(&x_v2);
}
