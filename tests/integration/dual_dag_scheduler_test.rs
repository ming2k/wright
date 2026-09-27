use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use wright::action::{
    ActionGraph, ActionId, ActionKind, ActionNode, ActionPlanner, ActionScheduler,
    PlannerOptions, SchedulerConfig,
};
use wright::config::GlobalConfig;
use wright::foundry::Foundry;
use wright::resolve::BuildExecutionPlan;
use wright_part::store::LocalPartStore;
use wright_cache::BuildCache;
use wright_registry::database::{InstalledDb, SessionContext};

#[test]
fn test_dual_dag_point_to_point_pipelining() {
    let mut build_set = HashSet::new();
    let mut deps_map = HashMap::new();
    let mut name_to_path = HashMap::new();

    // 3 packages: zlib (no deps), openssl (depends on zlib), curl (depends on openssl)
    build_set.insert("zlib".to_string());
    build_set.insert("openssl".to_string());
    build_set.insert("curl".to_string());

    deps_map.insert("zlib".to_string(), vec![]);
    deps_map.insert("openssl".to_string(), vec!["zlib".to_string()]);
    deps_map.insert("curl".to_string(), vec!["openssl".to_string()]);

    name_to_path.insert("zlib".to_string(), PathBuf::from("/plans/zlib"));
    name_to_path.insert("openssl".to_string(), PathBuf::from("/plans/openssl"));
    name_to_path.insert("curl".to_string(), PathBuf::from("/plans/curl"));

    let exec_plan = BuildExecutionPlan::for_testing(build_set, deps_map, name_to_path);

    let graph = ActionPlanner::plan(&exec_plan, &PlannerOptions::default()).unwrap();

    // 5 action atoms per package * 3 packages = 15 action nodes
    assert_eq!(graph.len(), 15);

    // Verify intra-package causal order: Build -> Seal -> VerifyAbi -> Deploy -> Commit
    let zlib_build = ActionId::build("zlib");
    let zlib_seal = ActionId::seal("zlib");
    let zlib_verify = ActionId::verify_abi("zlib");
    let zlib_deploy = ActionId::deploy("zlib");
    let zlib_commit = ActionId::commit("zlib");

    assert!(graph.dependencies_of(&zlib_seal).unwrap().contains(&zlib_build));
    assert!(graph.dependencies_of(&zlib_verify).unwrap().contains(&zlib_seal));
    assert!(graph.dependencies_of(&zlib_deploy).unwrap().contains(&zlib_verify));
    assert!(graph.dependencies_of(&zlib_commit).unwrap().contains(&zlib_deploy));

    // Verify point-to-point cross-package pipelining edge:
    // Build(openssl) depends on Deploy(zlib)
    let openssl_build = ActionId::build("openssl");
    assert!(graph.dependencies_of(&openssl_build).unwrap().contains(&zlib_deploy));

    // Build(curl) depends on Deploy(openssl)
    let openssl_deploy = ActionId::deploy("openssl");
    let curl_build = ActionId::build("curl");
    assert!(graph.dependencies_of(&curl_build).unwrap().contains(&openssl_deploy));

    // Topological sort must succeed with zero cycles
    let order = graph.topological_sort().unwrap();
    assert_eq!(order.len(), 15);
    // zlib build must be before openssl build
    let zlib_build_idx = order.iter().position(|id| id == &zlib_build).unwrap();
    let openssl_build_idx = order.iter().position(|id| id == &openssl_build).unwrap();
    let curl_build_idx = order.iter().position(|id| id == &curl_build).unwrap();
    assert!(zlib_build_idx < openssl_build_idx);
    assert!(openssl_build_idx < curl_build_idx);
}

#[tokio::test]
async fn test_action_scheduler_dry_run() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = GlobalConfig::default();
    config.general.db_path = tmp.path().join("wright.db");
    let config = Arc::new(config);

    let db = Arc::new(InstalledDb::open_in_memory().await.unwrap());
    let cache = Arc::new(BuildCache::new(tmp.path().join("store")));
    let part_store = Arc::new(LocalPartStore::new());
    let foundry = Arc::new(Foundry::new((*config).clone()));

    let scheduler = ActionScheduler::new(
        config,
        db,
        cache,
        part_store,
        foundry,
        SchedulerConfig {
            dry_run: true,
            ..Default::default()
        },
        tmp.path().join("root"),
        tmp.path().join("ledger"),
        SessionContext {
            id: "test-session".into(),
            command: "test".into(),
        },
    );

    let mut graph = ActionGraph::new();
    let a_build = ActionId::build("pkgA");
    graph.add_node(ActionNode::new(
        a_build.clone(),
        "pkgA",
        ActionKind::Build {
            plan_name: "pkgA".into(),
            clean: false,
            force: false,
            mvp: false,
        },
    ));

    let exec_plan = BuildExecutionPlan::for_testing(
        HashSet::from(["pkgA".to_string()]),
        HashMap::from([("pkgA".to_string(), vec![])]),
        HashMap::from([("pkgA".to_string(), PathBuf::from("/plans/pkgA"))]),
    );

    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let result = scheduler.execute(&mut graph, &exec_plan, cancel_rx).await;
    assert!(result.is_ok());
}
