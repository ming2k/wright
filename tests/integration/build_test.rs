use std::path::{Path, PathBuf};
use std::process::Command;

use wright::builder::Builder;
use wright::config::GlobalConfig;
use wright::part::part;
use wright::plan::manifest::{OutputConfig, PlanManifest};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn load_manifest_without_isolation(name: &str) -> (PlanManifest, PathBuf) {
    let manifest_path = fixture_path(name).join("plan.toml");
    let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
    for stage in manifest.lifecycle.values_mut() {
        stage.isolation = "none".to_string();
    }
    (manifest, manifest_path.parent().unwrap().to_path_buf())
}

#[tokio::test]
async fn test_build_hello_fixture() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Verify the binary was built
    assert!(result.output_dir.join("usr/bin/hello").exists());
}

#[tokio::test]
async fn test_build_and_archive_hello() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path = part::create_part(&result.output_dir, &manifest, output_dir.path()).unwrap();

    // Verify archive exists
    assert!(archive_path.exists());
    assert!(archive_path.to_string_lossy().ends_with(".wright.tar.zst"));

    // Verify we can read PARTINFO from it
    let partinfo = part::read_partinfo(&archive_path).unwrap();
    assert_eq!(partinfo.name, "hello");
    assert_eq!(partinfo.version, "1.0.0");
    assert_eq!(partinfo.release, 1);
    assert_eq!(partinfo.arch, "x86_64");

    // Verify we can extract it
    let extract_dir = tempfile::tempdir().unwrap();
    let (extracted_info, _hash) = part::extract_part(&archive_path, extract_dir.path()).unwrap();
    assert_eq!(extracted_info.name, "hello");
    assert!(extract_dir.path().join("usr/bin/hello").exists());
    assert!(extract_dir.path().join(".PARTINFO").exists());
    assert!(extract_dir.path().join(".FILELIST").exists());
}

#[tokio::test]
async fn test_archive_records_runtime_but_not_link_dependencies() {
    let manifest = PlanManifest::parse(
        r#"
name = "runtime-link-overlap"
version = "1.0.0"
release = 1
description = "test part"
license = "MIT"
arch = "x86_64"

build = []
link = ["zlib", "libffi"]

[[output]]
name = "runtime-link-overlap"
runtime_deps = ["openssl", "zlib"]

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${PART_DIR}/usr/bin/runtime-link-overlap
"""
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let plan_dir = tempfile::tempdir().unwrap();
    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            plan_dir.path(),
            Path::new("/"),
            &[],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    let output_dir = tempfile::tempdir().unwrap();
    let archive_path = part::create_part(&result.output_dir, &manifest, output_dir.path()).unwrap();
    let partinfo = part::read_partinfo(&archive_path).unwrap();

    assert_eq!(partinfo.runtime_deps, vec!["openssl", "zlib"]);

    let extract_dir = tempfile::tempdir().unwrap();
    part::extract_part(&archive_path, extract_dir.path()).unwrap();
    let partinfo = std::fs::read_to_string(extract_dir.path().join(".PARTINFO")).unwrap();
    assert!(partinfo.contains("runtime = [\"openssl\", \"zlib\"]"));
    assert!(!partinfo.contains("link ="));
}

#[tokio::test]
async fn test_canonical_and_split_build_variables_are_available() {
    let manifest = PlanManifest::parse(
        r#"
name = "split-vars"
version = "1.0.0"
release = 1
description = "test canonical plan variables"
license = "MIT"
arch = "x86_64"

build = []
link = []

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${PART_DIR}/usr/bin/${NAME}-${VERSION}
install -Dm644 /dev/null ${PART_DIR}/usr/share/doc/${NAME}
"""

[[output]]
name = "split-vars"

[[output]]
name = "split-vars-doc"
description = "doc output"
include = ["/usr/share/doc/.*"]
"#,
    )
    .unwrap();

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let plan_dir = tempfile::tempdir().unwrap();
    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            plan_dir.path(),
            Path::new("/"),
            &[],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(result.output_dir.join("usr/bin/split-vars-1.0.0").exists());
    assert!(result.split_part_dirs["split-vars-doc"]
        .join("usr/share/doc/split-vars")
        .exists());
}

#[tokio::test]
async fn test_lint_hello_fixture() {
    let manifest_path = fixture_path("hello").join("plan.toml");
    let manifest = PlanManifest::from_file(&manifest_path).unwrap();
    assert_eq!(manifest.plan.name, "hello");
    assert_eq!(manifest.plan.version.as_deref(), Some("1.0.0"));
}

#[tokio::test]
async fn test_lint_nginx_fixture() {
    let manifest_path = fixture_path("nginx").join("plan.toml");
    let manifest = PlanManifest::from_file(&manifest_path).unwrap();
    assert_eq!(manifest.plan.name, "nginx");
    // Nginx uses output metadata on the main output plus an extra doc output.
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => {
            assert!(parts.iter().any(|(n, _)| n == "nginx"));
            assert!(parts.iter().any(|(n, _)| n == "nginx-doc"));
            let (_, main) = parts.iter().find(|(n, _)| n == "nginx").unwrap();
            assert!(main.hooks.is_some());
            assert!(main.backup.is_some());
            assert_eq!(main.runtime_deps.len(), 3);
        }
        _ => panic!("expected Multi output config for nginx"),
    }
}

#[tokio::test]
async fn test_build_single_stage() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);

    // First do a full build so src/ directory exists
    builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Now run a single stage on the existing build tree
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &["prepare".to_string()],
            None,
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Running only prepare: hello.c should exist but hello binary should not
    // (output_dir is recreated fresh for single-stage runs)
    assert!(result.work_dir.join("hello.c").exists());
    assert!(!result.output_dir.join("usr/bin/hello").exists());
}

#[tokio::test]
async fn test_build_until_stage_runs_prior_stages_without_prior_workspace() {
    let (manifest, plan_dir) = load_manifest_without_isolation("hello");

    let mut config = GlobalConfig::default();
    let build_tmp = tempfile::tempdir().unwrap();
    config.build.build_dir = build_tmp.path().to_path_buf();

    let builder = Builder::new(config);
    let result = builder
        .build(
            &manifest,
            &plan_dir,
            Path::new("/"),
            &[],
            Some("staging"),
            false,
            false,
            &std::collections::HashMap::new(),
            false,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    assert!(result.work_dir.join("hello.c").exists());
    assert!(result.work_dir.join("hello").exists());
    assert!(result.output_dir.join("usr/bin/hello").exists());
    assert!(result.logs_dir.join("compile.log").exists());
    assert!(result.logs_dir.join("staging.log").exists());
}

#[tokio::test]
async fn test_print_parts_keeps_verbose_build_output_off_stdout() {
    let root = tempfile::tempdir().unwrap();
    let plans_dir = root.path().join("plans");
    let parts_dir = root.path().join("components");
    let cache_dir = root.path().join("cache");
    let db_dir = root.path().join("wright");
    let logs_dir = root.path().join("logs");
    let build_dir = root.path().join("build");
    std::fs::create_dir_all(&plans_dir).unwrap();
    std::fs::create_dir_all(&parts_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::create_dir_all(&build_dir).unwrap();

    let plan_dir = plans_dir.join("verbose-pipe-test");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("plan.toml"),
        r#"
name = "verbose-pipe-test"
version = "1.0.0"
release = 1
description = "verify stdout/stderr split for --print-parts"
license = "MIT"
arch = "x86_64"

build = []
link = []

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
echo LIVE-BUILD-OUTPUT
install -Dm755 /bin/sh ${PART_DIR}/usr/bin/verbose-pipe-test
"""
"#,
    )
    .unwrap();

    let config_path = root.path().join("wright.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
cache_dir = "{}"
db_path = "{}"
logs_dir = "{}"
executors_dir = "/etc/wright/executors"
assemblies_dir = "{}"

[build]
build_dir = "{}"
default_isolation = "none"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
            plans_dir.display(),
            parts_dir.display(),
            cache_dir.display(),
            db_dir.join("wright.db").display(),
            logs_dir.display(),
            root.path().join("assemblies").display(),
            build_dir.display(),
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("-v")
        .arg("build")
        .arg("verbose-pipe-test")
        .arg("--package")
        .arg("--print-parts")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wright build failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout_lines: Vec<_> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();

    assert_eq!(stdout_lines.len(), 1, "unexpected stdout: {stdout:?}");
    assert!(
        stdout_lines[0].ends_with(".wright.tar.zst"),
        "stdout should contain only archive paths: {stdout:?}"
    );
    assert!(
        !stdout.contains("LIVE-BUILD-OUTPUT"),
        "subprocess stdout leaked into stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("LIVE-BUILD-OUTPUT"),
        "expected live verbose build output on stderr: {stderr:?}"
    );
}

#[test]
fn test_until_stage_stops_before_packing_parts() {
    let root = tempfile::tempdir().unwrap();
    let plans_dir = root.path().join("plans");
    let parts_dir = root.path().join("parts");
    let cache_dir = root.path().join("cache");
    let db_dir = root.path().join("wright");
    let logs_dir = root.path().join("logs");
    let build_dir = root.path().join("build");
    std::fs::create_dir_all(&plans_dir).unwrap();
    std::fs::create_dir_all(&parts_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::create_dir_all(&build_dir).unwrap();

    let plan_dir = plans_dir.join("stop-at-staging");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("plan.toml"),
        r#"
name = "stop-at-staging"
version = "1.0.0"
release = 1
description = "verify --until-stage"
license = "MIT"
arch = "x86_64"

build = []
link = []

[lifecycle.prepare]
executor = "shell"
isolation = "none"
script = """
cat > hello.sh <<'EOF'
#!/bin/sh
echo stop-at-staging
EOF
chmod +x hello.sh
"""

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 hello.sh ${PART_DIR}/usr/bin/stop-at-staging
"""
"#,
    )
    .unwrap();

    let config_path = root.path().join("wright.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
source_dir = "{}"
db_path = "{}"
logs_dir = "{}"
executors_dir = "/etc/wright/executors"
assemblies_dir = "{}"

[build]
build_dir = "{}"
default_isolation = "none"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
            plans_dir.display(),
            parts_dir.display(),
            cache_dir.display(),
            db_dir.join("wright.db").display(),
            logs_dir.display(),
            root.path().join("assemblies").display(),
            build_dir.display(),
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("build")
        .arg("stop-at-staging")
        .arg("--until-stage")
        .arg("staging")
        .arg("--print-parts")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wright build failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "no part path should be printed when stopping before packing: {stdout:?}"
    );
    assert!(
        parts_dir.read_dir().unwrap().next().is_none(),
        "parts dir should stay empty when build stops after staging"
    );
    assert!(
        build_dir
            .join("stop-at-staging-1.0.0/staging/usr/bin/stop-at-staging")
            .exists(),
        "staged output should remain available for inspection"
    );
}

#[test]
fn test_build_resume_skips_already_completed_dependency_tasks() {
    let root = tempfile::tempdir().unwrap();
    let plans_dir = root.path().join("plans");
    let parts_dir = root.path().join("parts");
    let source_dir = root.path().join("sources");
    let state_dir = root.path().join("wright");
    let logs_dir = root.path().join("logs");
    let build_dir = root.path().join("build");
    std::fs::create_dir_all(&plans_dir).unwrap();
    std::fs::create_dir_all(&parts_dir).unwrap();
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::create_dir_all(&build_dir).unwrap();

    let counter_path = root.path().join("dep-counter");
    let signal_path = root.path().join("allow-main");

    let dep_dir = plans_dir.join("resume-dep");
    std::fs::create_dir_all(&dep_dir).unwrap();
    std::fs::write(
        dep_dir.join("plan.toml"),
        format!(
            r#"
name = "resume-dep"
version = "1.0.0"
release = 1
description = "dependency"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
printf x >> "{}"
install -Dm644 /dev/null ${{PART_DIR}}/usr/share/resume-dep
"""
"#,
            counter_path.display()
        ),
    )
    .unwrap();

    let main_dir = plans_dir.join("resume-main");
    std::fs::create_dir_all(&main_dir).unwrap();
    std::fs::write(
        main_dir.join("plan.toml"),
        format!(
            r#"
name = "resume-main"
version = "1.0.0"
release = 1
description = "main"
license = "MIT"
arch = "x86_64"

build = ["resume-dep"]
link = []

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
test -f "{}"
install -Dm644 /dev/null ${{PART_DIR}}/usr/share/resume-main
"""
"#,
            signal_path.display()
        ),
    )
    .unwrap();

    let config_path = root.path().join("wright.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
source_dir = "{}"
db_path = "{}"
logs_dir = "{}"
executors_dir = "/etc/wright/executors"
assemblies_dir = "{}"

[build]
build_dir = "{}"
default_isolation = "none"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
            plans_dir.display(),
            parts_dir.display(),
            source_dir.display(),
            state_dir.join("wright.db").display(),
            logs_dir.display(),
            root.path().join("assemblies").display(),
            build_dir.display(),
        ),
    )
    .unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("build")
        .arg("resume-dep")
        .arg("resume-main")
        .output()
        .unwrap();

    assert!(
        !first.status.success(),
        "first build should fail to leave a resumable session"
    );
    assert_eq!(
        std::fs::read_to_string(&counter_path).unwrap(),
        "x",
        "dependency should build exactly once before the failure"
    );

    std::fs::write(&signal_path, "ok").unwrap();

    let second = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("build")
        .arg("resume-dep")
        .arg("resume-main")
        .arg("--resume")
        .output()
        .unwrap();

    assert!(
        second.status.success(),
        "resume build failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&counter_path).unwrap(),
        "x",
        "resume should skip the already completed dependency build"
    );
}
