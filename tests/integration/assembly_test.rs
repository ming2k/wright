use std::fs;
use wright::config::AssembliesConfig;

#[tokio::test]
async fn test_assembly_loading_valid() {
    let temp = tempfile::tempdir().unwrap();
    let assemblies_dir = temp.path().join("assemblies");
    fs::create_dir_all(&assemblies_dir).unwrap();

    // Create a valid assembly file: base.toml
    fs::write(
        assemblies_dir.join("base.toml"),
        r#"
name = "base"
description = "Base system"
plans = ["glibc", "bash"]
"#,
    )
    .unwrap();

    let config = AssembliesConfig::load_all(&assemblies_dir).unwrap();
    assert_eq!(config.assemblies.len(), 1);
    assert!(config.assemblies.contains_key("base"));
    let base = config.assemblies.get("base").unwrap();
    assert_eq!(base.name, "base");
    assert_eq!(base.plans, vec!["glibc", "bash"]);
}

#[tokio::test]
async fn test_assembly_loading_mismatched_name() {
    let temp = tempfile::tempdir().unwrap();
    let assemblies_dir = temp.path().join("assemblies");
    fs::create_dir_all(&assemblies_dir).unwrap();

    // Create a mismatched assembly file: wrong.toml contains name = "base"
    fs::write(
        assemblies_dir.join("wrong.toml"),
        r#"
name = "base"
plans = ["glibc"]
"#,
    )
    .unwrap();

    let result = AssembliesConfig::load_all(&assemblies_dir);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("does not match file name 'wrong'"));
}

#[tokio::test]
async fn test_assembly_loading_legacy_format_fails() {
    let temp = tempfile::tempdir().unwrap();
    let assemblies_dir = temp.path().join("assemblies");
    fs::create_dir_all(&assemblies_dir).unwrap();

    // Create a legacy assembly file with [[assembly]] array
    fs::write(
        assemblies_dir.join("legacy.toml"),
        r#"
[[assembly]]
name = "legacy"
plans = ["old"]
"#,
    )
    .unwrap();

    let result = AssembliesConfig::load_all(&assemblies_dir);
    assert!(result.is_err());
    // The toml parser should fail because it expects name at the top level
    // but finds an 'assembly' array instead (or just missing 'name' if it's strict).
}

#[tokio::test]
async fn test_assembly_lint_logic() {
    use wright::commands::lint;
    use wright::config::GlobalConfig;

    let temp = tempfile::tempdir().unwrap();
    let plans_dir = temp.path().join("plans");
    let assemblies_dir = temp.path().join("assemblies");
    fs::create_dir_all(&plans_dir).unwrap();
    fs::create_dir_all(&assemblies_dir).unwrap();

    // 1. Create a valid plan
    let hello_dir = plans_dir.join("hello");
    fs::create_dir_all(&hello_dir).unwrap();
    fs::write(hello_dir.join("plan.toml"), "name = 'hello'\nversion = '1.0'\nrelease = 1\ndescription = 'test'\nlicense = 'MIT'\narch = 'x86_64'").unwrap();

    // 2. Create a valid assembly referencing existing plan
    fs::write(
        assemblies_dir.join("valid.toml"),
        "name = 'valid'\nplans = ['hello']",
    )
    .unwrap();

    // 3. Create an invalid assembly referencing non-existent plan
    fs::write(
        assemblies_dir.join("invalid.toml"),
        "name = 'invalid'\nplans = ['non-existent']",
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    config.general.plans_dir = plans_dir;
    config.general.assemblies_dir = assemblies_dir;

    // Linting valid assembly should succeed
    let result = lint::execute_lint(vec!["@valid".to_string()], false, &config).await;
    assert!(result.is_ok());

    // Linting invalid assembly should fail
    let result = lint::execute_lint(vec!["@invalid".to_string()], false, &config).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Lint failed"));
}
