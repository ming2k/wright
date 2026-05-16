use std::fs;
use std::path::PathBuf;

use wright::config::GlobalConfig;
use wright::database::InstalledDb;
use wright::operations::launch::{self, LaunchRequest, LaunchSource};

// ── Fixtures ─────────────────────────────────────────────────────────

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

#[derive(Default)]
struct FolioOpts<'a> {
    assumes: &'a [(&'a str, &'a str)],
    hooks: &'a [&'a str],
}

fn folio_content(name: &str, version: &str, plans: &[&str], opts: &FolioOpts) -> String {
    let mut out = format!(
        r#"[folio]
name = "{name}"
version = "{version}"
description = "test folio"

plans = [{}]
"#,
        plans.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(", "),
    );

    for (an, av) in opts.assumes {
        out.push_str(&format!(
            "\n[[provide]]\nname = \"{an}\"\nversion = \"{av}\"\n"
        ));
    }
    for script in opts.hooks {
        out.push_str(&format!(
            "\n[[hook]]\nstage = \"post-launch\"\nscript = \"{script}\"\n"
        ));
    }
    out
}

fn write_plan(plans_dir: &std::path::Path, name: &str) {
    let dir = plans_dir.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("plan.toml"), simple_plan_toml(name)).unwrap();
}

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

fn folio_req(path: PathBuf, dry_run: bool, force: bool) -> LaunchRequest {
    LaunchRequest {
        source: LaunchSource::Folio(path),
        dry_run,
        force,
    }
}

fn targets_req(
    plans_dir: Option<PathBuf>,
    folios_dir: Option<PathBuf>,
    targets: Vec<String>,
    dry_run: bool,
    force: bool,
) -> LaunchRequest {
    LaunchRequest {
        source: LaunchSource::Targets {
            plans_dir,
            folios_dir,
            targets,
        },
        dry_run,
        force,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn refuses_root_slash() {
    let config = GlobalConfig::default();
    let db_path = PathBuf::from("/tmp/wright-launch-test-does-not-exist.db");
    let root_dir = PathBuf::from("/");

    let err = launch::execute_launch(
        targets_req(None, None, vec!["x".into()], false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{err}").contains("refuses to fill `/`"), "{err}");
}

#[tokio::test]
async fn dry_run_has_no_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-a");
    write_plan(&config.general.plans_dir, "simple-b");

    let folio_path = temp.path().join("test.toml");
    fs::write(
        &folio_path,
        folio_content("test", "1", &["simple-a", "simple-b"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    launch::execute_launch(
        folio_req(folio_path, true, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    // Dry-run must not touch the target.
    assert!(!root_dir.exists(), "dry-run must not create the target root");
    assert!(!db_path.exists(), "dry-run must not create the target db");
}

#[tokio::test]
async fn launches_folio() {
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

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    assert!(root_dir.join("usr/share/simple-a").exists());
    assert!(
        root_dir
            .join("var/lib/wright/plans/simple-a/plan.toml")
            .exists()
    );

    let cfg = fs::read_to_string(root_dir.join("etc/wright/wright.toml")).unwrap();
    assert!(cfg.contains("plans_dir     = \"/var/lib/wright/plans\""));
    assert!(cfg.contains("store_dir     = \"/var/lib/wright/store\""));
    assert!(cfg.contains("source_dir    = \"/var/lib/wright/sources\""));

    assert!(root_dir.join("var/lib/wright/folios/test.toml").exists());
}

#[tokio::test]
async fn launches_plans_mode() {
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "simple-c");

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    launch::execute_launch(
        targets_req(
            Some(config.general.plans_dir.clone()),
            None,
            vec!["simple-c".into()],
            false,
            false,
        ),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
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
async fn resolves_folio_from_configured_folios_dir() {
    // `@core` resolves from the configured `folios_dir` — a peer of
    // `plans_dir`, never nested inside it.
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "plan-x");
    fs::write(
        config.general.folios_dir.join("core.toml"),
        folio_content("core", "1", &["plan-x"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    launch::execute_launch(
        targets_req(None, None, vec!["@core".into()], false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    assert!(root_dir.join("usr/share/plan-x").exists());
    assert!(root_dir.join("var/lib/wright/folios/core.toml").exists());
}

#[tokio::test]
async fn resolves_folio_from_cli_override() {
    // `--folios <DIR>` overrides folios_dir for one-off launches.
    let temp = tempfile::tempdir().unwrap();
    let config = test_config(temp.path());

    write_plan(&config.general.plans_dir, "plan-y");

    let adhoc_folios = temp.path().join("adhoc-folios");
    fs::create_dir_all(&adhoc_folios).unwrap();
    fs::write(
        adhoc_folios.join("core.toml"),
        folio_content("core", "1", &["plan-y"], &FolioOpts::default()),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    launch::execute_launch(
        targets_req(
            None,
            Some(adhoc_folios),
            vec!["@core".into()],
            false,
            false,
        ),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    assert!(root_dir.join("usr/share/plan-y").exists());
    assert!(root_dir.join("var/lib/wright/folios/core.toml").exists());
}

#[tokio::test]
async fn converges_on_rerun() {
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

    launch::execute_launch(
        folio_req(folio_path.clone(), false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();
    assert!(root_dir.join("usr/share/simple-d").exists());

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();
    assert!(root_dir.join("usr/share/simple-d").exists());
}

#[tokio::test]
async fn registers_provides() {
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

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    let db = InstalledDb::open(&db_path).await.unwrap();
    let linux = db.get_part("linux").await.unwrap().expect("linux part");
    assert_eq!(linux.origin, wright::database::Origin::External);
    assert_eq!(
        db.get_plan("linux").await.unwrap().unwrap().version,
        "6.12.0"
    );
    assert_eq!(
        db.get_plan("bash").await.unwrap().unwrap().version,
        "5.2"
    );
}

#[tokio::test]
async fn runs_post_launch_hook() {
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
                hooks: &[
                    "mkdir -p $ROOT/var/service && ln -s /etc/sv/sshd $ROOT/var/service/sshd",
                ],
                ..Default::default()
            },
        ),
    )
    .unwrap();

    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    #[cfg(unix)]
    {
        let sshd = root_dir.join("var/service/sshd");
        assert!(std::fs::symlink_metadata(&sshd).is_ok());
        assert_eq!(
            std::fs::read_link(&sshd).unwrap().to_string_lossy(),
            "/etc/sv/sshd"
        );
    }
}

#[tokio::test]
async fn empty_targets_error() {
    let temp = tempfile::tempdir().unwrap();
    let config = GlobalConfig::default();
    let root_dir = temp.path().join("target");
    let db_path = temp.path().join("target-db.db");

    let err = launch::execute_launch(
        targets_req(None, None, vec![], false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap_err();
    assert!(format!("{err}").contains("nothing to do"), "{err}");
}

#[tokio::test]
async fn launches_multiple_plans() {
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

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    for p in ["p1", "p2", "p3"] {
        assert!(root_dir.join(format!("usr/share/{p}")).exists());
    }
}

#[tokio::test]
async fn creates_target_skeleton() {
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

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    for dir in [
        "var/lib/wright",
        "var/lib/wright/parts",
        "var/lib/wright/store",
        "var/lib/wright/staging",
        "var/lib/wright/lock",
        "var/lib/wright/plans",
        "var/lib/wright/folios",
        "var/lib/wright/sources",
        "var/log/wright",
        "var/tmp/wright",
        "etc/wright",
    ] {
        assert!(root_dir.join(dir).exists(), "missing skeleton dir: {dir}");
    }
}

#[tokio::test]
async fn concurrent_targets_dont_interfere() {
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

    let (res_a, res_b) = tokio::join!(
        launch::execute_launch(
            folio_req(folio_a, false, false),
            &config_a,
            &db_a,
            &root_a,
            0,
            false
        ),
        launch::execute_launch(
            folio_req(folio_b, false, false),
            &config_b,
            &db_b,
            &root_b,
            0,
            false
        ),
    );
    res_a.unwrap();
    res_b.unwrap();

    assert!(root_a.join("usr/share/concurrent-a").exists());
    assert!(root_b.join("usr/share/concurrent-b").exists());
    assert!(!root_a.join("usr/share/concurrent-b").exists());
    assert!(!root_b.join("usr/share/concurrent-a").exists());
}

#[tokio::test]
async fn incremental_convergence() {
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

    launch::execute_launch(
        folio_req(folio_path.clone(), false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();
    assert!(root_dir.join("usr/share/incr-a").exists());
    assert!(!root_dir.join("usr/share/incr-b").exists());

    fs::write(
        &folio_path,
        folio_content("incr", "1", &["incr-a", "incr-b"], &FolioOpts::default()),
    )
    .unwrap();

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    assert!(root_dir.join("usr/share/incr-a").exists());
    assert!(root_dir.join("usr/share/incr-b").exists());
}

#[tokio::test]
async fn target_wright_toml_loads_clean() {
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

    launch::execute_launch(
        folio_req(folio_path, false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();

    let target_config = root_dir.join("etc/wright/wright.toml");
    let loaded = GlobalConfig::load(Some(&target_config)).unwrap();
    assert_eq!(loaded.general.plans_dir, PathBuf::from("/var/lib/wright/plans"));
    assert_eq!(loaded.general.folios_dir, PathBuf::from("/var/lib/wright/folios"));
    assert_eq!(loaded.general.parts_dir, PathBuf::from("/var/lib/wright/parts"));
    assert_eq!(loaded.general.store_dir, PathBuf::from("/var/lib/wright/store"));
    assert_eq!(loaded.general.source_dir, PathBuf::from("/var/lib/wright/sources"));
    assert_eq!(loaded.general.db_path, PathBuf::from("/var/lib/wright/wright.db"));
    assert_eq!(loaded.build.forge_dir, PathBuf::from("/var/tmp/wright/workshop"));
}

#[tokio::test]
async fn force_rebuild_succeeds() {
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

    launch::execute_launch(
        folio_req(folio_path.clone(), false, false),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();
    assert!(root_dir.join("usr/share/force-plan").exists());

    launch::execute_launch(
        folio_req(folio_path, false, true),
        &config,
        &db_path,
        &root_dir,
        0,
        false,
    )
    .await
    .unwrap();
    assert!(root_dir.join("usr/share/force-plan").exists());
}
