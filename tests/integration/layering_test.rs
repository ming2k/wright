//! Stage working-tree semantics (ADR-0037): `/build` is a real directory
//! tree, so filesystem operations that OverlayFS rejects must just work.
//! The canonical regression: cargo creates `target/` via a temp-dir rename
//! inside the source tree, which overlayfs fails with EXDEV when the parent
//! directory lives in a lower layer (and `redirect_dir` is unavailable
//! inside user namespaces, so no mount option can lift the restriction).

use std::process::Command;

fn isolated_config(root: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let plans_dir = root.path().join("plans");
    let parts_dir = root.path().join("parts");
    let source_dir = root.path().join("sources");
    let db_dir = root.path().join("state");
    let logs_dir = root.path().join("logs");
    let forge_dir = root.path().join("build");
    for dir in [
        &plans_dir,
        &parts_dir,
        &source_dir,
        &db_dir,
        &logs_dir,
        &forge_dir,
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }

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
forge_dir = "{}"
default_isolation = "relaxed"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
            plans_dir.display(),
            parts_dir.display(),
            source_dir.display(),
            db_dir.join("wright.db").display(),
            logs_dir.display(),
            root.path().join("assemblies").display(),
            forge_dir.display(),
        ),
    )
    .unwrap();
    (config_path, forge_dir)
}

/// A later stage renames a directory inside a tree that an earlier stage
/// produced (the cargo `target<random>` → `target` pattern).  With the
/// superseded overlay design this failed with EXDEV; with a real working
/// tree it is an ordinary rename.
#[test]
fn stage_working_tree_allows_directory_renames() {
    let root = tempfile::tempdir().unwrap();
    let (config_path, forge_dir) = isolated_config(&root);

    let plan_dir = root.path().join("plans/dir-rename");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(
        plan_dir.join("plan.toml"),
        r#"
name = "dir-rename"
version = "1.0.0"
release = 1
description = "directory renames must work in stage working trees"
license = "MIT"
arch = "x86_64"

link_deps = []

[pipeline.prepare]
executor = "shell"
isolation = "relaxed"
script = """
mkdir -p source/demo
echo original > source/demo/file.txt
"""

[pipeline.compile]
executor = "shell"
isolation = "relaxed"
script = """
cd source/demo
mkdir target.tmp
echo built > target.tmp/artifact.o
mv target.tmp target
"""

[pipeline.staging]
executor = "shell"
isolation = "relaxed"
script = """
install -Dm644 source/demo/target/artifact.o "${STAGING_DIR}/usr/share/demo/artifact.o"
"""
"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(&config_path)
        .arg("build")
        .arg("dir-rename")
        .arg("--until-stage")
        .arg("staging")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wright build failed: stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let build_root = forge_dir.join("dir-rename-1.0.0");
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap();

    // The renamed directory's content reached the staging deliverable…
    assert_eq!(
        read(&build_root.join("staging/usr/share/demo/artifact.o")),
        "built\n"
    );
    // …was harvested into the compile layer…
    assert_eq!(
        read(&build_root.join("layers/03-compile/source/demo/target/artifact.o")),
        "built\n"
    );
    // …and sits alongside the earlier stage's file in the merged base.
    assert_eq!(
        read(&build_root.join("base/source/demo/file.txt")),
        "original\n"
    );
    assert_eq!(
        read(&build_root.join("base/source/demo/target/artifact.o")),
        "built\n"
    );
}
