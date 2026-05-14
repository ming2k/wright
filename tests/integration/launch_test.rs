use std::fs;
use std::path::PathBuf;

use wright::config::GlobalConfig;
use wright::database::InstalledDb;
use wright::operations::launch::{self, LaunchRequest};

/// Minimal plan content for a shell-only plan (no compiler required).
fn simple_plan_toml(name: &str) -> String {
    format!(
        r#"
name = "{name}"
version = "1.0.0"
release = 1
description = "simple test plan"
license = "MIT"
arch = "x86_64"

[pipeline.staging]
executor = "shell"
isolation = "none"
script = "install -Dm644 /dev/null ${{STAGING_DIR}}/usr/share/{name}"
"#
    )
}

struct FolioOpts<'a> {
    assumes: &'a [(&'a str, &'a str)],
    hostname: Option<&'a str>,
    timezone: Option<&'a str>,
    locale: Option<&'a str>,
    services: &'a [&'a str],
}

#[allow(clippy::derivable_impls)]
impl Default for FolioOpts<'_> {
    fn default() -> Self {
        Self {
            assumes: &[],
            hostname: None,
            timezone: None,
            locale: None,
            services: &[],
        }
    }
}

/// A folio manifest that references named plans with optional assumes and config.
fn folio_content(name: &str, version: &str, plans: &[&str], opts: &FolioOpts) -> String {
    let mut out = format!(
        r#"[folio]
name = "{name}"
version = "{version}"
description = "test folio"
arch = "x86_64"

plans = [{}]
"#,
        plans
            .iter()
            .map(|p| format!("\"{}\"", p))
            .collect::<Vec<_>>()
            .join(", ")
    );

    for (an, av) in opts.assumes {
        out.push_str(&format!(
            "\n[[provide]]\nname = \"{}\"\nversion = \"{}\"\n",
            an, av
        ));
    }

    if opts.hostname.is_some()
        || opts.timezone.is_some()
        || opts.locale.is_some()
        || !opts.services.is_empty()
    {
        out.push_str("[config]\n");
        if let Some(h) = opts.hostname {
            out.push_str(&format!("hostname = \"{}\"\n", h));
        }
        if let Some(tz) = opts.timezone {
            out.push_str(&format!("timezone = \"{}\"\n", tz));
        }
        if let Some(l) = opts.locale {
            out.push_str(&format!("locale = \"{}\"\n", l));
        }
        if !opts.services.is_empty() {
            out.push_str(&format!(
                "services = [{}]\n",
                opts.services
                    .iter()
                    .map(|s| format!("\"{}\"", s))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    out
}

/// Create a plan directory with plan.toml inside `plans_dir`.
fn write_plan(plans_dir: &std::path::Path, name: &str) {
    let dir = plans_dir.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("plan.toml"), simple_plan_toml(name)).unwrap();
}

/// Set up a GlobalConfig with paths redirected into a temp directory.
fn test_config(temp: &std::path::Path) -> GlobalConfig {
    let plans = temp.join("plans");
    let folios = temp.join("folios");
    let parts = temp.join("parts");
    let store = temp.join("store");
    let sources = temp.join("sources");
    let db_path = temp.join("wright.db");
    let logs = temp.join("logs");
    let forge = temp.join("forge");

    for d in [&plans, &folios, &parts, &store, &sources, &logs, &forge] {
        fs::create_dir_all(d).unwrap();
    }

    let mut config = GlobalConfig::default();
    config.general.plans_dir = plans;
    config.general.folios_dir = folios;
    config.general.parts_dir = parts;
    config.general.store_dir = store;
    config.general.source_dir = sources;
    config.general.db_path = db_path;
    config.general.logs_dir = logs;
    config.build.forge_dir = forge;
    config.build.default_isolation = "none".to_string();
    config
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_launch_refuses_root_slash() {
    let config = GlobalConfig::default();
    let db_path = PathBuf::from("/tmp/wright-launch-test-does-not-exist.db");
    let root_dir = PathBuf::from("/");

    let req = LaunchRequest {
        folio: None,
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    let err = launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("refuses to fill `/`"),
        "expected refusal message, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_launch_folio_dry_run() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-a");
    write_plan(&config.general.plans_dir, "simple-b");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content(
            "test",
            "1",
            &["simple-a", "simple-b"],
            &FolioOpts::default(),
        ),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: true,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    // Skeleton is created even in dry-run (it happens before folio dispatch).
    assert!(root_dir.join("var/lib/wright/plans").exists());
    // But no deploy should have happened.
    assert!(!root_dir.join("usr/share/simple-a").exists());
    assert!(!root_dir.join("usr/share/simple-b").exists());
}

#[tokio::test]
async fn test_launch_folio_basic() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-a");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content("test", "1", &["simple-a"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    // Verify deployment output exists.
    assert!(root_dir.join("usr/share/simple-a").exists());

    // Verify plan was synced into the target.
    assert!(
        root_dir
            .join("var/lib/wright/plans/simple-a/plan.toml")
            .exists()
    );

    // Verify target config was written.
    let target_config = root_dir.join("etc/wright/wright.toml");
    assert!(target_config.exists());
    let cfg_text = fs::read_to_string(&target_config).unwrap();
    assert!(cfg_text.contains("plans_dir = \"/var/lib/wright/plans\""));
    assert!(cfg_text.contains("arch = \"x86_64\""));

    // Verify the folio was synced into the target.
    assert!(root_dir.join("var/lib/wright/folios/test.toml").exists());
}

#[tokio::test]
async fn test_launch_plans_mode() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-c");

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: None,
        plans: Some(config.general.plans_dir.clone()),
        plan_targets: vec!["simple-c".to_string()],
        dry_run: false,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    assert!(root_dir.join("usr/share/simple-c").exists());
    assert!(
        root_dir
            .join("var/lib/wright/plans/simple-c/plan.toml")
            .exists()
    );
}

#[tokio::test]
async fn test_launch_convergence() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-d");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content("test", "1", &["simple-d"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    // First launch.
    let req = LaunchRequest {
        folio: Some(folio_path.clone()),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    assert!(root_dir.join("usr/share/simple-d").exists());

    // Second launch — should converge without error.
    let req2 = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    let result = launch::execute_launch(req2, &config, &db_path, &root_dir, 0, false).await;
    assert!(
        result.is_ok(),
        "convergence re-run should succeed, got: {:?}",
        result.err()
    );

    // File should still exist (not removed).
    assert!(root_dir.join("usr/share/simple-d").exists());
}

#[tokio::test]
async fn test_launch_assumptions_registered() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-e");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content(
            "test",
            "1",
            &["simple-e"],
            &FolioOpts {
                assumes: &[("linux", "6.12.0"), ("bash", "5.2")],
                ..Default::default()
            },
        ),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    // Verify assumptions are in the target database.
    let db = InstalledDb::open(&db_path).await.unwrap();

    let linux_part = db.get_part("linux").await.unwrap();
    assert!(linux_part.is_some(), "linux should be assumed");
    assert_eq!(
        linux_part.unwrap().origin,
        wright::database::Origin::External
    );
    let linux_plan = db.get_plan("linux").await.unwrap();
    assert!(linux_plan.is_some(), "linux plan should be registered");
    assert_eq!(linux_plan.unwrap().version, "6.12.0");

    let bash_part = db.get_part("bash").await.unwrap();
    assert!(bash_part.is_some(), "bash should be assumed");
    let bash_plan = db.get_plan("bash").await.unwrap();
    assert!(bash_plan.is_some(), "bash plan should be registered");
    assert_eq!(bash_plan.unwrap().version, "5.2");
}

#[tokio::test]
async fn test_launch_config_applied() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-f");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content(
            "test",
            "1",
            &["simple-f"],
            &FolioOpts {
                hostname: Some("wrightbox"),
                timezone: Some("Europe/London"),
                locale: Some("en_GB.UTF-8"),
                services: &["sshd"],
                ..Default::default()
            },
        ),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    // Verify hostname.
    let hostname = fs::read_to_string(root_dir.join("etc/hostname")).unwrap();
    assert_eq!(hostname, "wrightbox\n");

    // Verify locale.
    let locale = fs::read_to_string(root_dir.join("etc/locale.conf")).unwrap();
    assert_eq!(locale, "LANG=en_GB.UTF-8\n");

    // Verify timezone symlink (Unix only).
    #[cfg(unix)]
    {
        let target = root_dir.join("etc/localtime");
        let link_target = std::fs::read_link(&target).unwrap();
        assert_eq!(
            link_target.to_string_lossy(),
            "../usr/share/zoneinfo/Europe/London"
        );
    }

    // Verify runit service symlink.
    #[cfg(unix)]
    {
        let svc = root_dir.join("var/service");
        assert!(svc.exists());
        let sshd = svc.join("sshd");
        if sshd.exists() {
            let link = std::fs::read_link(&sshd).unwrap();
            assert_eq!(link.to_string_lossy(), "/etc/sv/sshd");
        }
    }
}

#[tokio::test]
async fn test_launch_nothing_to_do_errors() {
    let temp = tempfile::tempdir().unwrap();
    let config = GlobalConfig::default();
    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: None,
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    let err = launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("nothing to do"),
        "expected 'nothing to do' error, got: {}",
        msg
    );
}

#[tokio::test]
async fn test_launch_multiple_plans() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "p1");
    write_plan(&config.general.plans_dir, "p2");
    write_plan(&config.general.plans_dir, "p3");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content("test", "1", &["p1", "p2", "p3"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    assert!(root_dir.join("usr/share/p1").exists());
    assert!(root_dir.join("usr/share/p2").exists());
    assert!(root_dir.join("usr/share/p3").exists());
}

#[tokio::test]
async fn test_launch_target_skeleton_structure() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-g");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content("test", "1", &["simple-g"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    for dir in &[
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/staging",
        "var/lib/wright/lock",
        "var/lib/wright/plans",
        "var/lib/wright/folios",
        "var/log/wright",
        "etc/wright",
    ] {
        assert!(
            root_dir.join(dir).exists(),
            "expected skeleton directory {} to exist",
            dir
        );
    }
}

// ── Concurrent launch: two independent targets must not interfere ─────

#[tokio::test]
async fn test_launch_concurrent_targets() {
    let temp_a = tempfile::tempdir().unwrap();
    let temp_b = tempfile::tempdir().unwrap();

    let config_a = test_config(temp_a.path());
    let config_b = test_config(temp_b.path());

    write_plan(&config_a.general.plans_dir, "concurrent-a");
    write_plan(&config_b.general.plans_dir, "concurrent-b");

    let folio_a = temp_a.path().join("folio-a.toml");
    let folio_b = temp_b.path().join("folio-b.toml");
    fs::write(
        &folio_a,
        folio_content("fa", "1", &["concurrent-a"], &FolioOpts::default()),
    )
    .unwrap();
    fs::write(
        &folio_b,
        folio_content("fb", "1", &["concurrent-b"], &FolioOpts::default()),
    )
    .unwrap();

    let root_a = temp_a.path().join("target");
    let root_b = temp_b.path().join("target");
    let db_a = temp_a.path().join("target.db");
    let db_b = temp_b.path().join("target.db");

    let req_a = LaunchRequest {
        folio: Some(folio_a),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    let req_b = LaunchRequest {
        folio: Some(folio_b),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };

    let (res_a, res_b) = tokio::join!(
        launch::execute_launch(req_a, &config_a, &db_a, &root_a, 0, false),
        launch::execute_launch(req_b, &config_b, &db_b, &root_b, 0, false),
    );

    assert!(
        res_a.is_ok(),
        "target A should launch successfully: {:?}",
        res_a.err()
    );
    assert!(
        res_b.is_ok(),
        "target B should launch successfully: {:?}",
        res_b.err()
    );

    // Each target received its own plan.
    assert!(root_a.join("usr/share/concurrent-a").exists());
    assert!(root_b.join("usr/share/concurrent-b").exists());

    // No cross-contamination between targets.
    assert!(
        !root_a.join("usr/share/concurrent-b").exists(),
        "target A must not contain plan B"
    );
    assert!(
        !root_b.join("usr/share/concurrent-a").exists(),
        "target B must not contain plan A"
    );

    // Each target has its own independent database.
    let db_a = InstalledDb::open(&db_a).await.unwrap();
    let db_b = InstalledDb::open(&db_b).await.unwrap();
    assert!(
        db_a.get_plan("concurrent-a").await.unwrap().is_some(),
        "target A db should record concurrent-a"
    );
    assert!(
        db_b.get_plan("concurrent-b").await.unwrap().is_some(),
        "target B db should record concurrent-b"
    );
}

// ── Incremental convergence: adding a plan to a folio after initial launch ─

#[tokio::test]
async fn test_launch_incremental_convergence() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "incr-a");
    write_plan(&config.general.plans_dir, "incr-b");

    let folio_path = temp.path().join("folio.toml");
    fs::write(
        &folio_path,
        folio_content("incr", "1", &["incr-a"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target.db");

    // First launch — only plan A.
    let req1 = LaunchRequest {
        folio: Some(folio_path.clone()),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    launch::execute_launch(req1, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    assert!(root_dir.join("usr/share/incr-a").exists());
    assert!(!root_dir.join("usr/share/incr-b").exists());

    // Update the folio on the host to also include plan B.
    fs::write(
        &folio_path,
        folio_content("incr", "1", &["incr-a", "incr-b"], &FolioOpts::default()),
    )
    .unwrap();

    // Second launch — must converge: keep A, add B.
    let req2 = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    launch::execute_launch(req2, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    assert!(
        root_dir.join("usr/share/incr-a").exists(),
        "plan A must survive incremental launch"
    );
    assert!(
        root_dir.join("usr/share/incr-b").exists(),
        "plan B must be added by incremental launch"
    );

    // Both plans are recorded in the target database.
    let db = InstalledDb::open(&db_path).await.unwrap();
    assert!(db.get_plan("incr-a").await.unwrap().is_some());
    assert!(db.get_plan("incr-b").await.unwrap().is_some());
}

// ── Target config is a valid, self-contained wright.toml ─────────────────

#[tokio::test]
async fn test_launch_target_config_loadable() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "cfg-plan");

    let folio_path = temp.path().join("folio.toml");
    fs::write(
        &folio_path,
        folio_content("cfg", "1", &["cfg-plan"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target.db");

    let req = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    launch::execute_launch(req, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    let target_config = root_dir.join("etc/wright/wright.toml");
    assert!(target_config.exists());

    // Load the target's own config as if we were running inside the target.
    let loaded = GlobalConfig::load(Some(&target_config)).unwrap();
    assert_eq!(
        loaded.general.plans_dir,
        std::path::PathBuf::from("/var/lib/wright/plans")
    );
    assert_eq!(
        loaded.general.folios_dir,
        std::path::PathBuf::from("/var/lib/wright/folios")
    );
    assert_eq!(
        loaded.general.parts_dir,
        std::path::PathBuf::from("/var/lib/wright/parts")
    );
    assert_eq!(
        loaded.general.db_path,
        std::path::PathBuf::from("/var/lib/wright/wright.db")
    );
    assert_eq!(
        loaded.build.forge_dir,
        std::path::PathBuf::from("/var/tmp/wright/workshop")
    );
}

// ── Force rebuild must succeed on an already-launched target ─────────────

#[tokio::test]
async fn test_launch_force_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "force-plan");

    let folio_path = temp.path().join("folio.toml");
    fs::write(
        &folio_path,
        folio_content("force", "1", &["force-plan"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target.db");

    // First launch.
    let req1 = LaunchRequest {
        folio: Some(folio_path.clone()),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: false,
    };
    launch::execute_launch(req1, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();
    assert!(root_dir.join("usr/share/force-plan").exists());

    // Second launch with --force.
    let req2 = LaunchRequest {
        folio: Some(folio_path),
        plans: None,
        plan_targets: vec![],
        dry_run: false,
        force: true,
    };
    launch::execute_launch(req2, &config, &db_path, &root_dir, 0, false)
        .await
        .unwrap();

    // Output should still be present.
    assert!(root_dir.join("usr/share/force-plan").exists());

    // Database should still be consistent.
    let db = InstalledDb::open(&db_path).await.unwrap();
    assert!(db.get_plan("force-plan").await.unwrap().is_some());
}
