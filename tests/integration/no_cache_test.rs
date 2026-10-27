//! Cold installs (`--no-cache`, ADR-0057): purge the pure caches, then
//! rebuild from freshly fetched sources, ignoring every derived reuse layer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn wright(config: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_wright"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Wright Test"])
        .args(["-c", "user.email=wright@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {:?} failed", args);
}

fn write_config(temp: &Path) -> PathBuf {
    let config_path = temp.join("wright.toml");
    fs::write(
        &config_path,
        format!(
            r#"[general]
arch = "x86_64"
plans_dir = "{}"
parts_dir = "{}"
store_dir = "{}"
source_dir = "{}"
db_path = "{}"
logs_dir = "{}"
executors_dir = "/etc/wright/executors"
assemblies_dir = "{}"

[build]
forge_dir = "{}"
default_isolation = "none"
ccache = false

[network]
download_timeout = 300
retry_count = 3
"#,
            temp.join("plans").display(),
            temp.join("parts").display(),
            temp.join("store").display(),
            temp.join("sources").display(),
            temp.join("wright/wright.db").display(),
            temp.join("logs").display(),
            temp.join("assemblies").display(),
            temp.join("build").display(),
        ),
    )
    .unwrap();
    config_path
}

/// A cache root that contains the part inventory must be rejected at config
/// load time, before any command can delete through it.
#[test]
fn test_overlapping_storage_layout_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let config = temp.path().join("wright.toml");
    fs::write(
        &config,
        format!(
            r#"[general]
source_dir = "{}"
store_dir = "{}"
parts_dir = "{}"
db_path = "{}"
"#,
            cache.display(),
            temp.path().join("store").display(),
            cache.join("parts").display(),
            temp.path().join("wright.db").display(),
        ),
    )
    .unwrap();

    let (ok, stdout, stderr) = wright(&config, &["build", "anything"]);
    assert!(!ok, "an overlapping layout must be rejected");
    let output = format!("{stdout}{stderr}");
    assert!(
        output.contains("storage layout conflict"),
        "expected a layout conflict, got: {output}"
    );
    assert!(
        output.contains("general.source_dir") && output.contains("general.parts_dir"),
        "the error must name both conflicting keys: {output}"
    );
}

/// The scenario this feature exists for: a plan pinned to a named git ref keeps
/// its snapshot forever, so a warm install cannot see the ref move. A cold
/// build/install must re-resolve it, purge both pure caches, bypass the charge
/// marker and CAS, and (for install) deploy the new content.
#[test]
fn test_cold_build_and_install_refresh_a_moved_git_ref() {
    let temp = tempfile::tempdir().unwrap();
    for dir in [
        "root",
        "plans",
        "parts",
        "store",
        "sources",
        "wright",
        "logs",
        "build",
        "assemblies",
        "upstream",
    ] {
        fs::create_dir_all(temp.path().join(dir)).unwrap();
    }

    let upstream = temp.path().join("upstream");
    git(&upstream, &["init"]);
    git(&upstream, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    fs::write(upstream.join("a.txt"), "v1\n").unwrap();
    git(&upstream, &["add", "a.txt"]);
    git(&upstream, &["commit", "-m", "v1"]);

    let plan_dir = temp.path().join("plans/git-pkg");
    fs::create_dir_all(&plan_dir).unwrap();
    fs::write(
        plan_dir.join("plan.toml"),
        format!(
            r#"
name = "git-pkg"
version = "1.0.0"
release = 1
description = "packages a file from a git source"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "git"
url = "{}"
ref = "main"

[pipeline.staging]
executor = "shell"
isolation = "none"
script = "install -Dm644 upstream-*/a.txt ${{STAGING_DIR}}/etc/payload"
"#,
            upstream.display()
        ),
    )
    .unwrap();

    let config = write_config(temp.path());
    let root = temp.path().join("root");
    let root_arg = root.display().to_string();
    let deployed = root.join("etc/payload");
    let staging = temp.path().join("build/git-pkg-1.0.0/staging/etc/payload");
    let sources = temp.path().join("sources");
    let store = temp.path().join("store");
    let parts = temp.path().join("parts");

    // 1. Warm install pins the snapshot at v1.
    let (ok, stdout, stderr) = wright(&config, &["install", "--root", &root_arg, "git-pkg"]);
    assert!(ok, "install failed: stdout={stdout} stderr={stderr}");
    assert_eq!(fs::read_to_string(&deployed).unwrap(), "v1\n");

    // 2. The ref moves upstream. A warm install must not see it: the snapshot
    //    is an explicit pin (ADR-0054), and its digest did not change.
    fs::write(upstream.join("a.txt"), "v2\n").unwrap();
    git(&upstream, &["add", "a.txt"]);
    git(&upstream, &["commit", "-m", "v2"]);

    let (ok, stdout, stderr) = wright(&config, &["install", "--root", &root_arg, "git-pkg"]);
    assert!(ok, "warm install failed: stdout={stdout} stderr={stderr}");
    assert_eq!(
        fs::read_to_string(&deployed).unwrap(),
        "v1\n",
        "a warm install must keep the pinned snapshot"
    );

    // Seed an unrelated source, an unrelated CAS entry, and an unrelated part
    // archive. The purge must delete the first two and preserve the last.
    let unrelated_source = sources.join("unrelated-cache.bin");
    let unrelated_store = store.join("ffffffffffffffff-unrelated.part");
    let unrelated_part = parts.join("keepme.wright.tar.zst");
    fs::write(&unrelated_source, b"stale source").unwrap();
    fs::write(&unrelated_store, b"stale cas").unwrap();
    fs::write(&unrelated_part, b"inventory").unwrap();

    // 3a. A plain `--dry-run` is a real build preview too: it resolves the
    //     same plan and touches nothing.
    let (ok, stdout, stderr) = wright(&config, &["build", "git-pkg", "--dry-run"]);
    assert!(ok, "build dry run failed: stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("batch 1"),
        "a dry run must resolve the execution plan: {stdout}"
    );
    assert!(unrelated_source.exists() && unrelated_store.exists());

    // 3b. `--no-cache --dry-run` reports the purge and resolves the real batch
    //     plan, but deletes nothing.
    let (ok, stdout, stderr) = wright(&config, &["build", "git-pkg", "--no-cache", "--dry-run"]);
    assert!(ok, "cold dry run failed: stdout={stdout} stderr={stderr}");
    assert!(
        format!("{stdout}{stderr}").contains("would purge"),
        "expected a purge preview: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("batch 1"),
        "a dry run must resolve the execution plan, not just echo targets: {stdout}"
    );
    assert!(
        unrelated_source.exists(),
        "a dry run must not delete sources"
    );
    assert!(
        unrelated_store.exists(),
        "a dry run must not delete CAS entries"
    );

    // 3c. A target that does not resolve must fail before the purge runs: a
    //     typo cannot be allowed to destroy shared caches.
    let (ok, stdout, stderr) = wright(&config, &["build", "does-not-exist", "--no-cache"]);
    assert!(!ok, "an unknown target must fail: {stdout}{stderr}");
    assert!(
        unrelated_source.exists() && unrelated_store.exists(),
        "target resolution must happen before the purge"
    );

    // 4. Cold build re-fetches the ref, purges both pure caches, and rebuilds
    //    without deploying.
    let (ok, stdout, stderr) = wright(&config, &["build", "git-pkg", "--no-cache"]);
    assert!(ok, "cold build failed: stdout={stdout} stderr={stderr}");
    assert_eq!(
        fs::read_to_string(&staging).unwrap(),
        "v2\n",
        "the build must use the re-fetched source; stdout={stdout} stderr={stderr}"
    );
    assert_eq!(
        fs::read_to_string(&deployed).unwrap(),
        "v1\n",
        "a build must not deploy"
    );
    assert!(
        !unrelated_source.exists(),
        "the purge must clear the source cache"
    );
    assert!(
        !unrelated_store.exists(),
        "the purge must clear the CAS store even for build (a later install would otherwise resurrect the old artifact)"
    );
    assert!(
        unrelated_part.exists(),
        "parts_dir archives are inventory, never a purge target"
    );

    // 5. Cold install purges again, re-fetches, rebuilds, and deploys.
    fs::write(&unrelated_source, b"stale source 2").unwrap();
    fs::write(&unrelated_store, b"stale cas 2").unwrap();
    let (ok, stdout, stderr) = wright(
        &config,
        &["install", "--root", &root_arg, "--no-cache", "git-pkg"],
    );
    assert!(ok, "cold install failed: stdout={stdout} stderr={stderr}");
    assert_eq!(
        fs::read_to_string(&deployed).unwrap(),
        "v2\n",
        "a cold install must deploy the re-fetched content; stdout={stdout} stderr={stderr}"
    );
    assert!(
        !unrelated_source.exists(),
        "the purge must clear the source cache"
    );
    assert!(
        !unrelated_store.exists(),
        "the purge must clear the CAS store"
    );
    assert!(unrelated_part.exists(), "parts_dir archives must survive");
    assert!(
        fs::read_dir(&store)
            .unwrap()
            .flatten()
            .any(|entry| entry.path().is_file()),
        "the cold build repopulates the CAS with what it just built"
    );

    // 6. Cold `upgrade` of an explicit target is not filtered by staleness:
    //    the ref moved again, and the plan must be selected even though its
    //    version triple and plan.toml are unchanged.
    fs::write(upstream.join("a.txt"), "v3\n").unwrap();
    git(&upstream, &["add", "a.txt"]);
    git(&upstream, &["commit", "-m", "v3"]);
    let (ok, stdout, stderr) = wright(
        &config,
        &["upgrade", "--root", &root_arg, "--no-cache", "git-pkg"],
    );
    assert!(ok, "cold upgrade failed: stdout={stdout} stderr={stderr}");
    assert!(
        !stdout.contains("already up to date"),
        "a cold upgrade must select the target regardless of staleness: {stdout}"
    );
    assert_eq!(
        fs::read_to_string(&deployed).unwrap(),
        "v3\n",
        "a cold upgrade must deploy the re-fetched content; stdout={stdout} stderr={stderr}"
    );

    // 7. `upgrade all --no-cache` selects every installed plan, not just the
    //    outdated ones.
    let (ok, stdout, stderr) = wright(
        &config,
        &[
            "upgrade",
            "--root",
            &root_arg,
            "all",
            "--no-cache",
            "--dry-run",
        ],
    );
    assert!(
        ok,
        "cold upgrade dry run failed: stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("git-pkg"),
        "cold `upgrade all` must list the installed plan: {stdout}"
    );
}
