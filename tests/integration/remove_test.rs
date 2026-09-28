//! Removal must never touch content Wright does not own.
//!
//! Ownership is tracked per exact path, so a single-owner directory entry says
//! nothing about the files inside it. These tests pin the invariant that
//! removing a part deletes only its own tracked files, preserves config files
//! on disk, and leaves any directory — owned or not — alone while it still
//! holds content Wright never registered.

use std::path::{Path, PathBuf};

use wright::config::GlobalConfig;
use wright::database::InstalledDb;
use wright::database::SessionContext;
use wright::foundry::{BuildOptions, Foundry};
use wright::part::archive;
use wright::plan::manifest::PlanManifest;
use wright::transaction;

/// Build a part that owns `/etc/pkg.d/pkg.conf` (declared config, so it is
/// preserved on removal) plus a plain binary under `/usr/bin`.
async fn create_test_archive(name: &str) -> PathBuf {
    let tmp = tempfile::tempdir().unwrap();
    let plan_dir = tmp.path().to_path_buf();

    let plan_toml = format!(
        r#"
name = "{name}"
version = "1.0.0"
release = 1
arch = "x86_64"
description = "test part"
license = "MIT"

[[output]]
name = "{name}"
description = "test output"
backup = ["/etc/pkg.d/pkg.conf"]

[pipeline.staging]
isolation = "none"
script = """
install -Dm644 /etc/hostname ${{STAGING_DIR}}/etc/pkg.d/pkg.conf
install -Dm755 /bin/true ${{STAGING_DIR}}/usr/bin/{name}-bin
"""
"#,
    );

    std::fs::write(plan_dir.join("plan.toml"), plan_toml).unwrap();

    let manifest = PlanManifest::from_file(&plan_dir.join("plan.toml")).unwrap();
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

    let persistent = std::env::temp_dir().join(format!(
        "test-remove-{}-{}.wright.tar.zst",
        name,
        std::process::id()
    ));
    std::fs::copy(&archive, &persistent).unwrap();
    persistent
}

fn session() -> SessionContext {
    SessionContext {
        id: "test".into(),
        command: "test".into(),
    }
}

/// Deploying a part registers the directories it created as owned entries. A
/// later removal must not relocate those directories into the rollback backup:
/// ownership is per-path, so moving a directory would carry away every
/// untracked file inside it, and the backup `TempDir` would then delete them.
#[tokio::test]
async fn test_remove_preserves_untracked_files_in_owned_directories() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("pkg").await;

    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    // The part owns these directories; nothing else claims them.
    let config_dir = root.path().join("etc/pkg.d");
    let bin_dir = root.path().join("usr/bin");
    assert!(config_dir.is_dir(), "config dir should exist after deploy");
    assert!(bin_dir.is_dir(), "bin dir should exist after deploy");

    // Admin drops their own tuning file into a directory Wright owns, and puts
    // a foreign file next to /etc/passwd-style content Wright never registered.
    let admin_dropin = config_dir.join("90-admin-tuning.conf");
    std::fs::write(&admin_dropin, "vm.swappiness=1").unwrap();
    let foreign = root.path().join("etc/foreign.conf");
    std::fs::write(&foreign, "not tracked by wright").unwrap();

    transaction::remove_part(&db, "pkg", root.path(), false, session())
        .await
        .unwrap();

    // The part's own non-config file is gone, and its binary is gone.
    assert!(
        !bin_dir.join("pkg-bin").exists(),
        "tracked binary should be removed"
    );

    // Untracked content must survive, wherever it lives.
    assert!(
        admin_dropin.exists(),
        "untracked admin drop-in inside an owned directory was destroyed"
    );
    assert_eq!(
        std::fs::read_to_string(&admin_dropin).unwrap(),
        "vm.swappiness=1"
    );
    assert!(
        foreign.exists(),
        "untracked file inside an owned directory was destroyed"
    );

    // The declared config file is preserved on disk per the removal contract.
    let config_file = config_dir.join("pkg.conf");
    assert!(
        config_file.exists(),
        "config file should be preserved on removal"
    );

    // A directory still holding foreign content cannot be removed, and must be
    // left in place rather than relocated into a backup that gets deleted.
    assert!(
        config_dir.is_dir(),
        "owned directory holding untracked content was removed"
    );

    // `/usr/bin` is empty once the tracked binary is gone, so removing it is
    // correct — this is the directory that Phase 2 is allowed to delete.
    assert!(
        !bin_dir.exists(),
        "empty owned directory should be cleaned up"
    );

    assert!(db.get_part("pkg").await.unwrap().is_none());

    let _ = std::fs::remove_file(&archive);
}

/// The config-preservation guarantee must hold on the filesystem, not merely in
/// the log stream. A `remove.config_preserved` event that fires while the file
/// is being relocated elsewhere is a false safety signal.
#[tokio::test]
async fn test_remove_config_file_survives_on_disk() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("cfgpkg").await;

    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    // Simulate an admin editing the config after install.
    let config_file = root.path().join("etc/pkg.d/pkg.conf");
    std::fs::write(&config_file, "admin edited value").unwrap();

    let part = db.get_part("cfgpkg").await.unwrap().unwrap();
    let files = db.get_files(part.id).await.unwrap();
    let config_entry = files
        .iter()
        .find(|f| f.path == "/etc/pkg.d/pkg.conf")
        .expect("config path should be registered");
    assert!(
        config_entry.is_config,
        "declared backup path should be marked is_config"
    );

    transaction::remove_part(&db, "cfgpkg", root.path(), false, session())
        .await
        .unwrap();

    assert!(
        config_file.exists(),
        "config file vanished despite being marked is_config"
    );
    assert_eq!(
        std::fs::read_to_string(&config_file).unwrap(),
        "admin edited value",
        "config file content was altered during removal"
    );

    let _ = std::fs::remove_file(&archive);
}

/// Removing a part leaves its declared config on disk. That residue must be
/// discoverable afterwards, not silently orphaned: the removal history row
/// carries the preserved paths.
#[tokio::test]
async fn test_removal_records_config_residue_in_history() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("residue").await;
    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    transaction::remove_part(&db, "residue", root.path(), false, session())
        .await
        .unwrap();

    let history = db.get_history(Some("residue")).await.unwrap();
    let removal = history
        .iter()
        .find(|h| matches!(h.action, wright::database::HistoryAction::Remove))
        .expect("removal history should exist");
    let details = removal
        .details
        .as_deref()
        .expect("removal should record preserved config paths");
    assert!(
        details.contains("/etc/pkg.d/pkg.conf"),
        "residue should name the preserved config, got: {details}"
    );

    let _ = std::fs::remove_file(&archive);
}

/// A removal batch is atomic. A failure part-way through must restore every
/// file already torn down and leave the registry untouched — no part removed,
/// no history row left `pending`.
#[tokio::test]
async fn test_failed_batch_rolls_back_completely() {
    use wright::database::Dependency;

    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let first = create_test_archive("batcha").await;
    let second = create_test_archive("batchb").await;
    transaction::deploy_part(&db, &first, root.path(), false, session(), ledger.path())
        .await
        .unwrap();
    transaction::deploy_part(&db, &second, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    // A third part outside the batch depends on batchb, so removing batchb
    // without force fails — after batcha has already been torn down.
    db.provide_part("batchc", "1.0").await.unwrap();
    let batchb = db.get_part("batchb").await.unwrap().unwrap();
    let _ = batchb; // batchb is the dependency target
    let batchc = db.get_part("batchc").await.unwrap().unwrap();
    db.insert_dependencies(
        batchc.id,
        &[Dependency {
            name: "batchb".to_string(),
            version_constraint: None,
        }],
    )
    .await
    .unwrap();

    let a_bin = root.path().join("usr/bin/batcha-bin");
    assert!(a_bin.exists());

    let parts = vec![
        wright::transaction::PartRef {
            name: "batcha".into(),
            hash: None,
        },
        wright::transaction::PartRef {
            name: "batchb".into(),
            hash: None,
        },
    ];
    let mut batch = wright::transaction::RemovalBatch::begin(&db, root.path(), session(), &parts)
        .await
        .unwrap();

    let ignored = std::collections::HashSet::new();
    batch.remove_one("batcha", true, &ignored).await.unwrap();
    assert!(
        !a_bin.exists(),
        "first part's binary should be torn down before the failure"
    );

    // batchb is blocked by batchc, which is not in the batch.
    let result = batch.remove_one("batchb", false, &ignored).await;
    assert!(result.is_err(), "second part should be blocked by batchc");
    batch.rollback().await.unwrap();

    // The first part's file is back, byte-for-byte.
    assert!(
        a_bin.exists(),
        "rollback must restore the first part's file"
    );
    // Neither part left the registry.
    assert!(db.get_part("batcha").await.unwrap().is_some());
    assert!(db.get_part("batchb").await.unwrap().is_some());

    // No history row is left pending.
    let history = db.get_history(None).await.unwrap();
    assert!(
        history
            .iter()
            .all(|h| !matches!(h.status, wright::database::HistoryStatus::Pending)),
        "no history row may be left pending after a rolled-back batch"
    );

    let _ = std::fs::remove_file(&first);
    let _ = std::fs::remove_file(&second);
}

/// A crash mid-removal leaves a journal. The next run's recovery must undo it,
/// because the registry still lists the parts.
#[tokio::test]
async fn test_crash_recovery_undoes_interrupted_removal() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("crashed").await;
    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    let bin = root.path().join("usr/bin/crashed-bin");
    assert!(bin.exists());

    // Simulate the crash: journal a backup but never commit or roll back, and
    // leak the transaction (its journal stays on disk). The intent records the
    // real hash, exactly as production does.
    let deployed = db.get_part("crashed").await.unwrap().unwrap();
    let parts = vec![wright::transaction::PartRef {
        name: "crashed".into(),
        hash: deployed.part_hash.clone(),
    }];
    let mut batch = wright::transaction::RemovalBatch::begin(&db, root.path(), session(), &parts)
        .await
        .unwrap();
    let ignored = std::collections::HashSet::new();
    batch.remove_one("crashed", true, &ignored).await.unwrap();
    assert!(!bin.exists(), "file should be torn down before the crash");
    std::mem::forget(batch); // leak: journal remains on disk

    // Recovery: registry still lists "crashed", so the journal is undone.
    let handled = transaction::recover_transactions(root.path(), &db)
        .await
        .unwrap();
    assert!(handled >= 1, "recovery should have handled the journal");
    assert!(
        bin.exists(),
        "recovery must restore the interrupted removal"
    );
    assert!(db.get_part("crashed").await.unwrap().is_some());

    let _ = std::fs::remove_file(&archive);
}

/// Recovery must discard a journal whose parts are already gone — that removal
/// committed, and the leftover backup store is garbage.
#[tokio::test]
async fn test_crash_recovery_discards_committed_removal() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("committed").await;
    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    // Simulate a crash *after* the registry commit but *before* the backup
    // store was cleaned: forge a journal for a part the registry no longer has.
    let rollback_dir = root.path().join("var/lib/wright/rollback/tx-committed");
    std::fs::create_dir_all(&rollback_dir).unwrap();
    let junk = rollback_dir.join("backup/usr/bin/ghost");
    std::fs::create_dir_all(junk.parent().unwrap()).unwrap();
    std::fs::write(&junk, b"stale").unwrap();
    std::fs::write(
        rollback_dir.join("journal.jsonl"),
        "{\"kind\":\"header\",\"intents\":[{\"action\":\"remove\",\"part_name\":\"ghost\",\"part_hash\":null}]}\n\
         {\"kind\":\"moved\",\"from\":\"/nonexistent/ghost\",\"to\":\"/nonexistent/b\"}\n",
    )
    .unwrap();

    let handled = transaction::recover_transactions(root.path(), &db)
        .await
        .unwrap();
    assert!(handled >= 1);
    assert!(
        !rollback_dir.exists(),
        "committed removal's backup store should be discarded"
    );

    let _ = std::fs::remove_file(&archive);
}

/// The race that made the old engine lose installed files: a crash between the
/// database write and journal cleanup. The journal still exists, but the
/// registry says the install committed — recovery must discard the journal
/// rather than undo a completed install.
#[tokio::test]
async fn test_crash_between_commit_and_journal_cleanup_keeps_install() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("raced").await;
    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    let bin = root.path().join("usr/bin/raced-bin");
    assert!(bin.exists());

    let installed = db.get_part("raced").await.unwrap().unwrap();
    let hash = installed.part_hash.clone().unwrap_or_default();

    // Forge the exact post-crash state: the install's journal is still on disk
    // (cleanup did not run), claiming a FileCreated for the installed binary.
    let tx_dir = root.path().join("var/lib/wright/rollback/tx-raced");
    std::fs::create_dir_all(&tx_dir).unwrap();
    std::fs::write(
        tx_dir.join("journal.jsonl"),
        format!(
            "{{\"kind\":\"header\",\"intents\":[{{\"action\":\"install\",\"part_name\":\"raced\",\"part_hash\":\"{hash}\"}}]}}\n\
             {{\"kind\":\"file_created\",\"path\":\"{}\"}}\n",
            bin.display()
        ),
    )
    .unwrap();

    let handled = transaction::recover_transactions(root.path(), &db)
        .await
        .unwrap();
    assert!(handled >= 1);
    assert!(
        bin.exists(),
        "a committed install must not have its files removed by recovery"
    );
    assert!(db.get_part("raced").await.unwrap().is_some());

    let _ = std::fs::remove_file(&archive);
}

/// A removal that starts a delivery transaction must record its operations, so
/// a crash leaves an APPLYING record with real ops. Without that, recovery
/// would see an empty op set, declare the delivery "completed with no ops", and
/// never settle the pending history row.
#[tokio::test]
async fn test_crashed_removal_delivery_is_settled_by_recovery() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("crasheddel").await;
    transaction::deploy_part(&db, &archive, root.path(), false, session(), ledger.path())
        .await
        .unwrap();

    // Simulate a removal that began a delivery, registered its op, moved into
    // APPLYING, then crashed before commit — modelled directly so we control
    // the exact state.
    let tx_id = wright::delivery::begin_delivery(&db, "remove crasheddel")
        .await
        .unwrap();
    wright::delivery::register_ops(
        &db,
        tx_id,
        &[(
            "crasheddel".to_string(),
            String::new(),
            "remove".to_string(),
            0,
            None,
        )],
    )
    .await
    .unwrap();
    wright::delivery::begin_applying(&db, tx_id).await.unwrap();

    // Recovery sees an APPLYING delivery with one PENDING op, resets it, and
    // rolls the delivery back.
    let recovered = wright::delivery::recover_if_needed(&db).await.unwrap();
    assert!(recovered, "recovery should have found the active delivery");

    let active = db.get_active_delivery().await.unwrap();
    assert!(
        active.is_none(),
        "recovery must clear the active delivery, not leave it stuck"
    );

    // The part is still installed and nothing was lost.
    assert!(db.get_part("crasheddel").await.unwrap().is_some());

    let _ = std::fs::remove_file(&archive);
}

#[tokio::test]
async fn test_execute_remove_force_allows_removing_depended_part() {
    let db = InstalledDb::open_in_memory().await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let ledger = tempfile::tempdir().unwrap();

    let archive = create_test_archive("hello").await;
    transaction::deploy_part(
        &db,
        &archive,
        root.path(),
        false,
        session(),
        ledger.path(),
    )
    .await
    .unwrap();

    // Register a dependent part that depends on 'hello'
    db.provide_part("consumer-app", "1.0").await.unwrap();
    let consumer = db.get_part("consumer-app").await.unwrap().unwrap();
    db.insert_dependencies(
        consumer.id,
        &[wright::database::Dependency {
            name: "hello".to_string(),
            version_constraint: None,
        }],
    )
    .await
    .unwrap();

    // Removing 'hello' without force must fail because 'consumer-app' depends on it
    let res_unforced = wright::operations::remove::execute_remove(
        &db,
        &["hello"],
        false,
        false,
        false,
        false,
        root.path(),
    )
    .await;
    assert!(res_unforced.is_err());
    let err_str = res_unforced.unwrap_err().to_string();
    assert!(err_str.contains("cannot remove: hello (required by consumer-app)"));
    assert!(db.get_part("hello").await.unwrap().is_some());

    // Removing 'hello' WITH force must succeed and remove it cleanly
    let res_forced = wright::operations::remove::execute_remove(
        &db,
        &["hello"],
        true,
        false,
        false,
        false,
        root.path(),
    )
    .await;
    assert!(res_forced.is_ok(), "forced execute_remove must succeed");
    assert!(db.get_part("hello").await.unwrap().is_none());
    assert!(!root.path().join("usr/bin/hello").exists());

    let _ = std::fs::remove_file(&archive);
}
