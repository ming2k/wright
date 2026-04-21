use std::fs;
use wright::config::AssembliesConfig;

#[test]
fn test_assembly_loading_valid() {
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

#[test]
fn test_assembly_loading_mismatched_name() {
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

#[test]
fn test_assembly_loading_legacy_format_fails() {
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
