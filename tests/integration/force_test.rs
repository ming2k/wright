use wright::builder::orchestrator::{resolve_build_set, MatchPolicy, ResolveOptions};
use wright::config::GlobalConfig;
use wright::database::{InstalledDb, NewPart, Origin};

#[tokio::test]
async fn test_apply_force_always_includes_explicit_targets() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("state").join("installed.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let db = InstalledDb::open(&db_path).await.unwrap();

    // 1. Simulate a part 'a' already installed
    db.insert_part(NewPart {
        name: "a",
        version: "1.0.0",
        release: 1,
        epoch: 0,
        description: "test",
        arch: "x86_64",
        license: "MIT",
        url: None,
        install_size: 100,
        part_hash: Some("oldhash"),
        install_scripts: None,
        origin: Origin::Manual,
        plan_name: None,
        plan_id: None,
    })
    .await
    .unwrap();
    drop(db);

    // 2. Setup a plan directory for 'a'
    let plans_dir = temp.path().join("plans");
    let a_dir = plans_dir.join("a");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::write(
        a_dir.join("plan.toml"),
        r#"
name = "a"
version = "1.0.0"
release = 1
epoch = 0
description = "test"
license = "MIT"
arch = "x86_64"
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    config.general.installed_db_path = db_path;
    config.general.plans_dir = plans_dir;

    // 3. Resolve without force (MatchPolicy::Outdated)
    // Since 1.0.0-1 is already installed and matches plan, it should be empty.
    let opts_no_force = ResolveOptions {
        match_policies: vec![MatchPolicy::Outdated],
        include_targets: true,
        preserve_targets: false,
        ..Default::default()
    };
    let build_set_1 = resolve_build_set(&config, vec!["a".to_string()], opts_no_force)
        .await
        .unwrap();
    assert!(
        build_set_1.is_empty(),
        "Without force, converged target should be skipped"
    );

    // 4. Resolve WITH force (preserve_targets: true)
    // Even if converged, it MUST be included.
    let opts_force = ResolveOptions {
        match_policies: vec![MatchPolicy::Outdated],
        include_targets: true,
        preserve_targets: true,
        ..Default::default()
    };
    let build_set_2 = resolve_build_set(&config, vec!["a".to_string()], opts_force)
        .await
        .unwrap();
    assert_eq!(
        build_set_2.len(),
        1,
        "With force/preserve_targets, target MUST be included"
    );
    assert_eq!(build_set_2[0], "a");
}
