use std::fs;
use tempfile::TempDir;
use wright::config::GlobalConfig;
use wright::database::{InstalledDb, NewPart, NewPlan, Origin};
use wright::resolve::{DepDomain, MatchPolicy, ResolveOptions, resolve_build_set};

struct TestEnv {
    _temp: TempDir,
    config: GlobalConfig,
}

impl TestEnv {
    async fn setup() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let plans = temp.path().join("plans");
        let parts = temp.path().join("parts");
        let state = temp.path().join("wright");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&plans).unwrap();
        fs::create_dir_all(&parts).unwrap();
        fs::create_dir_all(&state).unwrap();

        let db_path = state.join("wright.db");
        let ledger_dir = state.join("ledger");
        let db = InstalledDb::open(&db_path, Some(&ledger_dir))
            .await
            .unwrap();

        // 1. Install 3-tier chain: app -> libmid -> libbase (installed at v1.0.0)
        for (name, dep) in [
            ("libbase", None),
            ("libmid", Some("libbase")),
            ("app", Some("libmid")),
        ] {
            let plan_id = db
                .insert_plan(NewPlan {
                    name,
                    version: "1.0.0",
                    release: 1,
                    epoch: 0,
                    arch: "x86_64",
                })
                .await
                .unwrap();

            db.insert_part(NewPart {
                name,
                plan_id,
                part_hash: Some("hash1"),
                deploy_scripts: None,
                origin: Origin::Manual,
            })
            .await
            .unwrap();

            // On-disk plans have v2.0.0 (outdated relative to installed state)
            let dir = plans.join(name);
            fs::create_dir_all(&dir).unwrap();
            let link_deps_field = match dep {
                Some(d) => format!("link_deps = [\"{d}\"]\n"),
                None => String::new(),
            };
            fs::write(
                dir.join("plan.toml"),
                format!(
                    r#"name = "{name}"
version = "2.0.0"
release = 1
description = "{name} test plan"
license = "MIT"
arch = "x86_64"
{link_deps_field}
[pipeline.staging]
executor = "shell"
isolation = "none"
script = "echo ok"
"#
                ),
            )
            .unwrap();
        }

        // 2. Install reverse consumers: direct_consumer (depends on app) -> indirect_consumer (depends on direct_consumer)
        for (name, dep) in [
            ("direct_consumer", "app"),
            ("indirect_consumer", "direct_consumer"),
        ] {
            let plan_id = db
                .insert_plan(NewPlan {
                    name,
                    version: "1.0.0",
                    release: 1,
                    epoch: 0,
                    arch: "x86_64",
                })
                .await
                .unwrap();

            db.insert_part(NewPart {
                name,
                plan_id,
                part_hash: Some("hash1"),
                deploy_scripts: None,
                origin: Origin::Manual,
            })
            .await
            .unwrap();

            let dir = plans.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("plan.toml"),
                format!(
                    r#"name = "{name}"
version = "1.0.0"
release = 1
description = "{name} consumer plan"
license = "MIT"
arch = "x86_64"
link_deps = ["{dep}"]
[pipeline.staging]
executor = "shell"
isolation = "none"
script = "echo ok"
"#
                ),
            )
            .unwrap();
        }

        drop(db);

        let mut config = GlobalConfig::default();
        config.general.parts_dir = parts;
        config.general.db_path = db_path;
        config.general.plans_dir = plans;

        TestEnv {
            _temp: temp,
            config,
        }
    }
}

#[tokio::test]
async fn test_forward_chain_contained_by_default() {
    let env = TestEnv::setup().await;

    // Default targeted resolution: deps_depth = None -> dep_match_policies defaults to [Missing]
    let opts = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::empty(),
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Missing]),
        deps_depth: None,
        rdeps_depth: None,
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&env.config, vec!["app".to_string()], opts)
        .await
        .unwrap()
        .names;

    assert!(build_set.contains(&"app".to_string()));
    assert!(
        !build_set.contains(&"libmid".to_string()),
        "Satisfied dependency libmid must be contained by default"
    );
    assert!(
        !build_set.contains(&"libbase".to_string()),
        "Satisfied transitive dependency libbase must be contained by default"
    );
}

#[tokio::test]
async fn test_forward_chain_depth_1_expansion() {
    let env = TestEnv::setup().await;

    // deps_depth = Some(1): upgrade target + direct dependencies only
    let opts = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::empty(),
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Outdated]),
        deps_depth: Some(1),
        rdeps_depth: None,
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&env.config, vec!["app".to_string()], opts)
        .await
        .unwrap()
        .names;

    assert!(build_set.contains(&"app".to_string()));
    assert!(
        build_set.contains(&"libmid".to_string()),
        "Direct dependency libmid must be included when deps_depth=1"
    );
    assert!(
        !build_set.contains(&"libbase".to_string()),
        "Transitive dependency libbase at depth 2 must NOT be included when deps_depth=1"
    );
}

#[tokio::test]
async fn test_forward_chain_full_bottom_up_deep_upgrade() {
    let env = TestEnv::setup().await;

    // deps_depth = Some(0): unlimited depth (--deep) upgrades entire upstream chain bottom-up
    let opts = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::empty(),
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Outdated]),
        deps_depth: Some(0),
        rdeps_depth: None,
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&env.config, vec!["app".to_string()], opts)
        .await
        .unwrap()
        .names;

    assert!(build_set.contains(&"app".to_string()));
    assert!(
        build_set.contains(&"libmid".to_string()),
        "libmid must be in deep build set"
    );
    assert!(
        build_set.contains(&"libbase".to_string()),
        "libbase must be in deep build set (entire bottom-up chain)"
    );
}

#[tokio::test]
async fn test_reverse_dependents_1_hop_blast_radius() {
    let env = TestEnv::setup().await;

    // Upgrading 'app': rdeps_depth = Some(1) bounds rebuilds to direct consumers only
    let opts = ResolveOptions {
        deps: DepDomain::empty(),
        rdeps: DepDomain::LINK,
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Missing]),
        deps_depth: None,
        rdeps_depth: Some(1),
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&env.config, vec!["app".to_string()], opts)
        .await
        .unwrap()
        .names;

    assert!(build_set.contains(&"app".to_string()));
    assert!(
        build_set.contains(&"direct_consumer".to_string()),
        "1-hop direct consumer must be included when rdeps_depth=1"
    );
    assert!(
        !build_set.contains(&"indirect_consumer".to_string()),
        "2-hop indirect consumer must NOT be included when rdeps_depth=1 (blast radius bounded)"
    );
}

#[tokio::test]
async fn test_reverse_dependents_unlimited_depth() {
    let env = TestEnv::setup().await;

    // Upgrading 'app': rdeps_depth = Some(0) expands all reverse dependents transitively
    let opts = ResolveOptions {
        deps: DepDomain::empty(),
        rdeps: DepDomain::LINK,
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Missing]),
        deps_depth: None,
        rdeps_depth: Some(0),
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&env.config, vec!["app".to_string()], opts)
        .await
        .unwrap()
        .names;

    assert!(build_set.contains(&"app".to_string()));
    assert!(build_set.contains(&"direct_consumer".to_string()));
    assert!(
        build_set.contains(&"indirect_consumer".to_string()),
        "indirect_consumer must be included when rdeps_depth=0 (unlimited)"
    );
}

#[tokio::test]
async fn test_bidirectional_deep_upstream_with_1_hop_downstream_impact() {
    let env = TestEnv::setup().await;

    // Combined: deps_depth = Some(0) (deep bottom-up chain) AND rdeps_depth = Some(1) (1-hop impact)
    let opts = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::LINK,
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Outdated]),
        deps_depth: Some(0),
        rdeps_depth: Some(1),
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&env.config, vec!["app".to_string()], opts)
        .await
        .unwrap()
        .names;

    // Target
    assert!(build_set.contains(&"app".to_string()));

    // Upstream chain (deps)
    assert!(build_set.contains(&"libmid".to_string()));
    assert!(build_set.contains(&"libbase".to_string()));

    // Downstream direct consumer (1-hop rdeps)
    assert!(build_set.contains(&"direct_consumer".to_string()));

    // Downstream 2-hop consumer (must be excluded)
    assert!(
        !build_set.contains(&"indirect_consumer".to_string()),
        "indirect_consumer must be excluded because rdeps_depth=1"
    );
}
