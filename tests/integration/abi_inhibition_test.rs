use std::collections::BTreeSet;

use wright::part::abi::{
    AbiBreakReason, AbiCompatibility, ElfAbi, PartAbi, diff_abi, read_abi_info, write_abi_info,
};
use wright::plan::manifest::{AbiStability, PlanManifest};

#[test]
fn test_diff_abi_identical() {
    let mut symbols = BTreeSet::new();
    symbols.insert("crypto_init".to_string());
    symbols.insert("crypto_encrypt".to_string());

    let lib = ElfAbi {
        soname: Some("libcrypto.so.3".to_string()),
        needed: vec!["libc.so.6".to_string()],
        exported_symbols: symbols,
        abi_hash: "hash_v1".to_string(),
    };

    let mut old_abi = PartAbi::default();
    old_abi.libraries.insert("usr/lib/libcrypto.so.3".to_string(), lib);
    old_abi.recompute_overall_hash();

    let new_abi = old_abi.clone();
    let result = diff_abi(&old_abi, &new_abi);
    assert_eq!(result, AbiCompatibility::Identical);
    assert!(result.is_compatible());
}

#[test]
fn test_diff_abi_backward_compatible_superset() {
    let mut old_symbols = BTreeSet::new();
    old_symbols.insert("ssl_init".to_string());
    old_symbols.insert("ssl_connect".to_string());

    let mut new_symbols = old_symbols.clone();
    new_symbols.insert("ssl_connect_async".to_string()); // newly added function

    let old_lib = ElfAbi {
        soname: Some("libssl.so.3".to_string()),
        needed: vec!["libc.so.6".to_string(), "libcrypto.so.3".to_string()],
        exported_symbols: old_symbols,
        abi_hash: "hash_old".to_string(),
    };

    let new_lib = ElfAbi {
        soname: Some("libssl.so.3".to_string()),
        needed: vec!["libc.so.6".to_string(), "libcrypto.so.3".to_string()],
        exported_symbols: new_symbols,
        abi_hash: "hash_new".to_string(),
    };

    let mut old_abi = PartAbi::default();
    old_abi.libraries.insert("usr/lib/libssl.so.3".to_string(), old_lib);
    old_abi.recompute_overall_hash();

    let mut new_abi = PartAbi::default();
    new_abi.libraries.insert("usr/lib/libssl.so.3".to_string(), new_lib);
    new_abi.recompute_overall_hash();

    let result = diff_abi(&old_abi, &new_abi);
    assert_eq!(
        result,
        AbiCompatibility::CompatibleSuperset { added_symbols: 1 }
    );
    assert!(result.is_compatible());
}

#[test]
fn test_diff_abi_symbol_removal_breakage() {
    let mut old_symbols = BTreeSet::new();
    old_symbols.insert("xml_parse".to_string());
    old_symbols.insert("xml_free".to_string());

    let mut new_symbols = BTreeSet::new();
    new_symbols.insert("xml_parse".to_string()); // xml_free removed!

    let old_lib = ElfAbi {
        soname: Some("libxml2.so.2".to_string()),
        needed: vec![],
        exported_symbols: old_symbols,
        abi_hash: "hash_v1".to_string(),
    };

    let new_lib = ElfAbi {
        soname: Some("libxml2.so.2".to_string()),
        needed: vec![],
        exported_symbols: new_symbols,
        abi_hash: "hash_v2".to_string(),
    };

    let mut old_abi = PartAbi::default();
    old_abi.libraries.insert("usr/lib/libxml2.so.2".to_string(), old_lib);
    old_abi.recompute_overall_hash();

    let mut new_abi = PartAbi::default();
    new_abi.libraries.insert("usr/lib/libxml2.so.2".to_string(), new_lib);
    new_abi.recompute_overall_hash();

    let result = diff_abi(&old_abi, &new_abi);
    assert!(matches!(
        result,
        AbiCompatibility::Incompatible(AbiBreakReason::SymbolsRemoved { .. })
    ));
    assert!(!result.is_compatible());
}

#[test]
fn test_diff_abi_soname_bump_breakage() {
    let mut symbols = BTreeSet::new();
    symbols.insert("curl_easy_init".to_string());

    let old_lib = ElfAbi {
        soname: Some("libcurl.so.4".to_string()),
        needed: vec![],
        exported_symbols: symbols.clone(),
        abi_hash: "curl_v4".to_string(),
    };

    let new_lib = ElfAbi {
        soname: Some("libcurl.so.5".to_string()), // SONAME bumped from 4 to 5!
        needed: vec![],
        exported_symbols: symbols,
        abi_hash: "curl_v5".to_string(),
    };

    let mut old_abi = PartAbi::default();
    old_abi.libraries.insert("usr/lib/libcurl.so.4".to_string(), old_lib);
    old_abi.recompute_overall_hash();

    let mut new_abi = PartAbi::default();
    new_abi.libraries.insert("usr/lib/libcurl.so.5".to_string(), new_lib);
    new_abi.recompute_overall_hash();

    let result = diff_abi(&old_abi, &new_abi);
    assert!(matches!(
        result,
        AbiCompatibility::Incompatible(AbiBreakReason::SonameChanged { .. })
    ));
    assert!(!result.is_compatible());
}

#[test]
fn test_diff_abi_library_removed() {
    let mut symbols = BTreeSet::new();
    symbols.insert("sub_func".to_string());

    let old_lib = ElfAbi {
        soname: Some("libsub.so.1".to_string()),
        needed: vec![],
        exported_symbols: symbols,
        abi_hash: "sub_hash".to_string(),
    };

    let mut old_abi = PartAbi::default();
    old_abi.libraries.insert("usr/lib/libsub.so.1".to_string(), old_lib);
    old_abi.recompute_overall_hash();

    let new_abi = PartAbi::default(); // empty in new version!

    let result = diff_abi(&old_abi, &new_abi);
    assert!(matches!(
        result,
        AbiCompatibility::Incompatible(AbiBreakReason::LibraryRemoved(_))
    ));
    assert!(!result.is_compatible());
}

#[test]
fn test_diff_abi_no_shared_libraries() {
    let old_abi = PartAbi::default();
    let new_abi = PartAbi::default();

    let result = diff_abi(&old_abi, &new_abi);
    assert_eq!(
        result,
        AbiCompatibility::Incompatible(AbiBreakReason::NoSharedLibraries)
    );
    assert!(!result.is_compatible());
}

#[test]
fn test_plan_abi_manifest_parsing() {
    let toml_str = r#"
name = "boost"
version = "1.84.0"
release = 1
epoch = 0
description = "Boost C++ libraries"
license = "BSL-1.0"
arch = "x86_64"
abi_epoch = 3
abi_stability = "inlined"
"#;

    let manifest = PlanManifest::parse(toml_str).unwrap();
    assert_eq!(manifest.metadata.abi_epoch, Some(3));
    assert_eq!(manifest.metadata.abi_stability, AbiStability::Inlined);
}

#[test]
fn test_plan_abi_manifest_defaults() {
    let toml_str = r#"
name = "zlib"
version = "1.3.1"
release = 1
description = "Compression library"
license = "Zlib"
arch = "x86_64"
"#;

    let manifest = PlanManifest::parse(toml_str).unwrap();
    assert_eq!(manifest.metadata.abi_epoch, None);
    assert_eq!(manifest.metadata.abi_stability, AbiStability::Dynamic);
}

#[test]
fn test_abiinfo_serialization_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(".ABIINFO");

    let mut symbols = BTreeSet::new();
    symbols.insert("deflate".to_string());
    symbols.insert("inflate".to_string());

    let lib = ElfAbi {
        soname: Some("libz.so.1".to_string()),
        needed: vec!["libc.so.6".to_string()],
        exported_symbols: symbols,
        abi_hash: "zlib_hash_123".to_string(),
    };

    let mut original_abi = PartAbi::default();
    original_abi
        .libraries
        .insert("usr/lib/libz.so.1.3.1".to_string(), lib);
    original_abi.recompute_overall_hash();

    write_abi_info(&file, &original_abi).unwrap();
    let restored = read_abi_info(tmp.path()).unwrap().expect("loaded .ABIINFO");

    assert_eq!(original_abi, restored);
}

#[tokio::test]
async fn test_targeted_install_does_not_propagate_updates_to_satisfied_deps() {
    use std::fs;
    use wright::config::GlobalConfig;
    use wright::database::{InstalledDb, NewPart, NewPlan, Origin};
    use wright::resolve::{DepDomain, MatchPolicy, ResolveOptions, resolve_build_set};

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
    let db = InstalledDb::open(&db_path, Some(&ledger_dir)).await.unwrap();

    // 1. Simulate installed system state:
    // 'dep-satisfied' is installed at version 1.0.0
    let plan_id = db
        .insert_plan(NewPlan {
            name: "dep-satisfied",
            version: "1.0.0",
            release: 1,
            epoch: 0,
            arch: "x86_64",
        })
        .await
        .unwrap();

    db.insert_part(NewPart {
        name: "dep-satisfied",
        plan_id,
        part_hash: Some("dummy_hash_1"),
        deploy_scripts: None,
        origin: Origin::Manual,
    })
    .await
    .unwrap();

    // 2. In plans directory:
    // 'dep-satisfied' has a newer version 2.0.0 on disk!
    let dep_dir = plans.join("dep-satisfied");
    fs::create_dir_all(&dep_dir).unwrap();
    fs::write(
        dep_dir.join("plan.toml"),
        r#"
name = "dep-satisfied"
version = "2.0.0"
release = 1
description = "Satisfied dep with newer version"
license = "MIT"
arch = "x86_64"
[pipeline.staging]
executor = "shell"
isolation = "none"
script = "echo ok"
"#,
    )
    .unwrap();

    // 'dep-missing' is completely NOT installed
    let missing_dir = plans.join("dep-missing");
    fs::create_dir_all(&missing_dir).unwrap();
    fs::write(
        missing_dir.join("plan.toml"),
        r#"
name = "dep-missing"
version = "1.0.0"
release = 1
description = "Missing dep"
license = "MIT"
arch = "x86_64"
[pipeline.staging]
executor = "shell"
isolation = "none"
script = "echo ok"
"#,
    )
    .unwrap();

    // Target plan 'optics' depends on both 'dep-satisfied' and 'dep-missing'
    let optics_dir = plans.join("optics");
    fs::create_dir_all(&optics_dir).unwrap();
    fs::write(
        optics_dir.join("plan.toml"),
        r#"
name = "optics"
version = "1.0.0"
release = 1
description = "Target"
license = "MIT"
arch = "x86_64"
link_deps = ["dep-satisfied", "dep-missing"]
[pipeline.staging]
executor = "shell"
isolation = "none"
script = "echo ok"
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    config.general.parts_dir = parts;
    config.general.db_path = db_path.clone();
    config.general.plans_dir = plans.clone();

    // Drop db connection so resolve_build_set can acquire the lock
    drop(db);

    // 3. Resolve 'optics' with default contained dependency expansion
    // Target policies = Outdated, Dep policies = Missing
    let opts = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::empty(),
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Missing]),
        deps_depth: Some(0),
        rdeps_depth: Some(0),
        include_targets: true,
        preserve_targets: true,
    };

    let build_set = resolve_build_set(&config, vec!["optics".to_string()], opts)
        .await
        .unwrap()
        .names;

    println!("build_set = {:?}", build_set);

    // 'optics' must be in build set (explicit target)
    assert!(build_set.contains(&"optics".to_string()));
    // 'dep-missing' must be in build set (missing dependency required to build/run)
    assert!(build_set.contains(&"dep-missing".to_string()));
    // 'dep-satisfied' MUST NOT be in build set (already installed, update propagation contained!)
    assert!(
        !build_set.contains(&"dep-satisfied".to_string()),
        "Satisfied dependency should NOT be eagerly upgraded during targeted install, got: {:?}",
        build_set
    );

    // 4. Verify explicit operator override:
    // If the operator explicitly requests `--match=outdated`, then outdated deps ARE included!
    let opts_eager = ResolveOptions {
        deps: DepDomain::ALL,
        rdeps: DepDomain::empty(),
        match_policies: vec![MatchPolicy::Outdated],
        dep_match_policies: Some(vec![MatchPolicy::Outdated]),
        deps_depth: Some(0),
        rdeps_depth: Some(0),
        include_targets: true,
        preserve_targets: true,
    };

    let build_set_eager = resolve_build_set(&config, vec!["optics".to_string()], opts_eager)
        .await
        .unwrap()
        .names;

    assert!(
        build_set_eager.contains(&"dep-satisfied".to_string()),
        "When explicitly requested with --match=outdated, outdated deps must be included"
    );
}
