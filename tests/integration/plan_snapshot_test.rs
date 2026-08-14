use std::path::{Path, PathBuf};

use wright::database::{InstalledDb, SessionContext};
use wright::part::archive;
use wright::plan::manifest::PlanManifest;
use wright::{ledger, transaction};

fn session() -> SessionContext {
    SessionContext {
        id: "test".into(),
        command: "test".into(),
    }
}

/// Write a minimal valid plan.toml into `dir` and return its path.
fn write_plan(dir: &Path, release: u32) -> PathBuf {
    let path = dir.join("plan.toml");
    std::fs::write(
        &path,
        format!(
            r#"
name = "snap"
version = "1.0.0"
release = {release}
description = "plan snapshot integration test"
license = "MIT"
arch = "x86_64"

[pipeline.staging]
executor = "shell"
isolation = "none"
script = "true"
"#
        ),
    )
    .unwrap();
    path
}

/// Seal a one-file part for the plan at `plan_path` into a persistent file.
fn seal_archive(manifest: &PlanManifest) -> PathBuf {
    let staging = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
    std::fs::write(staging.path().join("usr/bin/snap"), "#!/bin/sh\n").unwrap();
    let out = tempfile::tempdir().unwrap();
    let archive = archive::create_part(staging.path(), manifest, out.path(), None).unwrap();

    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let persistent = std::env::temp_dir().join(format!(
        "plan-snapshot-integration-{}-{}.wright.tar.zst",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::copy(&archive, &persistent).unwrap();
    persistent
}

#[tokio::test]
async fn sealed_plan_source_survives_source_edit_and_deletion() {
    let workspace = tempfile::tempdir().unwrap();
    let plan_path = write_plan(workspace.path(), 1);
    let manifest = PlanManifest::from_file(&plan_path).unwrap();
    let expected_source = std::fs::read_to_string(&plan_path).unwrap();
    let checksum = manifest.plan_checksum.clone().unwrap();

    // The sealed archive carries the snapshot as a member.
    let archive_path = seal_archive(&manifest);
    let extract = tempfile::tempdir().unwrap();
    let _ = archive::extract_part(&archive_path, extract.path()).unwrap();
    assert_eq!(
        archive::read_plan_source(extract.path()).as_deref(),
        Some(expected_source.as_str())
    );

    // Deploy records the snapshot in the file ledger, keyed by plan checksum.
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    transaction::deploy_part(
        &db,
        &archive_path,
        root.path(),
        false,
        session(),
        ledger_dir.path(),
    )
    .await
    .unwrap();
    assert_eq!(
        ledger::plan_snapshot_source(ledger_dir.path(), "snap", &checksum).as_deref(),
        Some(expected_source.as_str())
    );

    // The snapshot file lives under the plan's ledger directory, named by
    // recording time and checksum.
    let snapshots_dir = ledger_dir.path().join("snap/snapshots");
    let names: Vec<String> = std::fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 1);
    assert!(
        names[0].ends_with(&format!("-{}.toml", checksum)),
        "unexpected snapshot filename: {}",
        names[0]
    );

    // The snapshot member is metadata, not payload: it never lands on disk.
    assert!(!root.path().join(".PLANSRC").exists());
    let part = db.get_part("snap").await.unwrap().unwrap();
    let files = db.get_files(part.id).await.unwrap();
    assert!(!files.iter().any(|f| f.path.contains(".PLANSRC")));

    // The plan source is edited and then deleted; the ledger still answers.
    std::fs::write(&plan_path, "release = 99\n").unwrap();
    std::fs::remove_file(&plan_path).unwrap();
    assert_eq!(
        ledger::plan_snapshot_source(ledger_dir.path(), "snap", &checksum).as_deref(),
        Some(expected_source.as_str())
    );
}

#[tokio::test]
async fn upgrade_registers_new_snapshot_and_retains_old() {
    let workspace = tempfile::tempdir().unwrap();
    let plan_path = write_plan(workspace.path(), 1);
    let manifest_v1 = PlanManifest::from_file(&plan_path).unwrap();
    let source_v1 = std::fs::read_to_string(&plan_path).unwrap();
    let archive_v1 = seal_archive(&manifest_v1);

    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    transaction::deploy_part(
        &db,
        &archive_v1,
        root.path(),
        false,
        session(),
        ledger_dir.path(),
    )
    .await
    .unwrap();

    // Bump the release and re-seal; the plan text changed, so the checksum
    // and snapshot must change with it.
    let plan_path = write_plan(workspace.path(), 2);
    let manifest_v2 = PlanManifest::from_file(&plan_path).unwrap();
    let source_v2 = std::fs::read_to_string(&plan_path).unwrap();
    let archive_v2 = seal_archive(&manifest_v2);

    transaction::upgrade_part(
        &db,
        &archive_v2,
        root.path(),
        false,
        false,
        session(),
        ledger_dir.path(),
    )
    .await
    .unwrap();

    let checksum_v1 = manifest_v1.plan_checksum.as_deref().unwrap();
    let checksum_v2 = manifest_v2.plan_checksum.as_deref().unwrap();
    assert_ne!(checksum_v1, checksum_v2);
    assert_eq!(
        ledger::plan_snapshot_source(ledger_dir.path(), "snap", checksum_v2).as_deref(),
        Some(source_v2.as_str())
    );
    // The ledger keeps history: the superseded snapshot stays retrievable.
    assert_eq!(
        ledger::plan_snapshot_source(ledger_dir.path(), "snap", checksum_v1).as_deref(),
        Some(source_v1.as_str())
    );

    // The plan row tracks the newest provenance.
    let plan = db.get_plan("snap").await.unwrap().unwrap();
    assert_eq!(plan.plan_checksum.as_deref(), Some(checksum_v2));
}
