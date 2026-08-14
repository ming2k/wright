use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::progress;
use wright_part::compression as compress;
use wright_part::store::sanitize_cache_filename;
use wright_plan::manifest::{PlanManifest, Source};

use super::git::git_snapshot_filename;
use super::{Charge, source_cache_filename};

impl Charge {
    // ------------------------------------------------------------------
    // Extract
    // ------------------------------------------------------------------

    pub(super) async fn extract(
        &self,
        manifest: &PlanManifest,
        dest_dir: &Path,
    ) -> Result<PathBuf> {
        let mut placed: HashSet<PathBuf> = HashSet::new();
        for source in &manifest.sources.entries {
            match source {
                Source::Git(git) => {
                    let processed_url = variables::process_uri(&git.url, manifest);
                    let git_ref = git
                        .r#ref
                        .as_deref()
                        .map(|r| variables::process_uri(r, manifest))
                        .unwrap_or_else(|| "HEAD".to_string());
                    let snapshot_name = git_snapshot_filename(&processed_url, &git_ref);
                    let final_dest = if let Some(ref sub) = git.extract_to {
                        let sub = variables::process_uri(sub, manifest);
                        dest_dir.join(&sub)
                    } else {
                        dest_dir.join(snapshot_name.trim_end_matches(".tar.zst"))
                    };
                    tokio::fs::create_dir_all(&final_dest)
                        .await
                        .map_err(WrightError::IoError)?;
                    if git.git_metadata {
                        // The build expects a working git repository (e.g. for
                        // `git submodule update --init`): clone from upstream
                        // straight into the work directory.
                        self.clone_git_source(
                            &processed_url,
                            &git_ref,
                            &final_dest,
                            &manifest.metadata.name,
                        )?;
                    } else {
                        debug!(
                            "Extracting git snapshot {} to {} ...",
                            snapshot_name,
                            final_dest.display()
                        );
                        let label = progress::source_label(&processed_url);
                        let _span = crate::cli_span!(
                            "Extracting",
                            "{} ({})",
                            label,
                            manifest.metadata.name
                        );
                        let snapshot = self.cache_dir.join(&snapshot_name);
                        compress::extract_part(&snapshot, &final_dest).map_err(|e| {
                            WrightError::context(
                                format!("failed to extract git snapshot {snapshot_name}"),
                                e,
                            )
                        })?;
                    }
                }
                Source::Http(http) => {
                    let processed_url = variables::process_uri(&http.url, manifest);
                    let filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_url)
                    });
                    let cache_path = self.cache_dir.join(&filename);
                    let final_dest = if let Some(ref sub) = http.extract_to {
                        let sub = variables::process_uri(sub, manifest);
                        let p = dest_dir.join(&sub);
                        tokio::fs::create_dir_all(&p)
                            .await
                            .map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.to_path_buf()
                    };
                    if is_part_file(&filename) {
                        let label = progress::source_label(&processed_url);
                        let _span = crate::cli_span!(
                            "Extracting",
                            "{} ({})",
                            label,
                            manifest.metadata.name
                        );
                        compress::extract_part(&cache_path, &final_dest).map_err(|e| {
                            WrightError::context(format!("failed to extract source {filename}"), e)
                        })?;
                    } else {
                        let dest_name = http
                            .r#as
                            .clone()
                            .unwrap_or_else(|| source_workdir_filename(&processed_url));
                        let dest = final_dest.join(&dest_name);
                        claim_workdir_dest(&mut placed, &dest)?;
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::context(format!("failed to copy non-archive source {dest_name} to work directory"), e)
                        })?;
                    }
                }
                Source::Local(local) => {
                    let processed_path = variables::process_uri(&local.path, manifest);
                    let filename = local.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_path)
                    });
                    let cache_path = self.cache_dir.join(&filename);
                    let final_dest = if let Some(ref sub) = local.extract_to {
                        let sub = variables::process_uri(sub, manifest);
                        let p = dest_dir.join(&sub);
                        tokio::fs::create_dir_all(&p)
                            .await
                            .map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.to_path_buf()
                    };
                    if is_part_file(&filename) {
                        let label = progress::source_label(&processed_path);
                        let _span = crate::cli_span!(
                            "Extracting",
                            "{} ({})",
                            label,
                            manifest.metadata.name
                        );
                        compress::extract_part(&cache_path, &final_dest).map_err(|e| {
                            WrightError::context(
                                format!("failed to extract local source {filename}"),
                                e,
                            )
                        })?;
                    } else {
                        let dest_name = local
                            .r#as
                            .clone()
                            .unwrap_or_else(|| source_workdir_filename(&processed_path));
                        let dest = final_dest.join(&dest_name);
                        claim_workdir_dest(&mut placed, &dest)?;
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::context(
                                format!(
                                    "failed to copy local source {dest_name} to work directory"
                                ),
                                e,
                            )
                        })?;
                    }
                }
            }
        }
        Ok(dest_dir.to_path_buf())
    }
}

/// Destination filename for a non-archive source placed in the work
/// directory: the source's own basename. The part-name prefix only exists to
/// namespace the shared source cache and must not leak into ${WORKDIR}.
fn source_workdir_filename(uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(basename)
}

/// Two sources of one plan must not resolve to the same work directory file.
fn claim_workdir_dest(placed: &mut HashSet<PathBuf>, dest: &Path) -> Result<()> {
    if placed.insert(dest.to_path_buf()) {
        Ok(())
    } else {
        Err(WrightError::ForgeError(format!(
            "two sources resolve to the same work directory file '{}'; rename one with `as`",
            dest.display()
        )))
    }
}

fn is_part_file(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".tgz")
        || filename.ends_with(".tar.xz")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.zst")
        || filename.ends_with(".tar.lz")
        || filename.ends_with(".zip")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Semaphore;

    use super::*;
    use crate::config::GlobalConfig;
    use wright_plan::manifest::PlanManifest;

    #[tokio::test]
    async fn extract_extracts_git_snapshot_without_metadata() {
        let root = tempfile::tempdir().unwrap();
        let upstream = root.path().join("upstream");
        let mut upstream_repo = gix::init(&upstream).unwrap();
        {
            let mut config = upstream_repo.config_snapshot_mut();
            config
                .set_value(&gix::config::tree::User::NAME, "Wright Test")
                .unwrap();
            config
                .set_value(&gix::config::tree::User::EMAIL, "wright@example.invalid")
                .unwrap();
        }
        let blob_id = upstream_repo
            .write_blob(b"from tagged source\n")
            .unwrap()
            .detach();
        let tree = gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "payload.txt".into(),
                oid: blob_id,
            }],
        };
        let tree_id = upstream_repo.write_object(&tree).unwrap().detach();
        let commit_id = upstream_repo
            .commit("HEAD", "initial", tree_id, Vec::<gix::ObjectId>::new())
            .unwrap();
        upstream_repo
            .reference(
                "refs/tags/v1.0.0",
                commit_id,
                gix::refs::transaction::PreviousValue::Any,
                "test tag",
            )
            .unwrap();

        let source_url = upstream.to_str().unwrap();
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = sources_dir.clone();
        let charge = Charge::new(&config, Arc::new(Semaphore::new(1)));

        // Populate the source cache the way the fetch stage does.
        let snapshot = sources_dir.join(git_snapshot_filename(source_url, "v1.0.0"));
        charge
            .fetch_git_snapshot(source_url, Some("v1.0.0"), &snapshot, "test")
            .unwrap();

        let manifest = PlanManifest::parse(&format!(
            r#"
name = "git-tag-source"
version = "1.0.0"
release = 1
description = "test git tag source"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "git"
url = "{source_url}"
ref = "v${{VERSION}}"
extract_to = "source"
"#
        ))
        .unwrap();

        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        charge.extract(&manifest, &dest_dir).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.join("source/payload.txt")).unwrap(),
            "from tagged source\n"
        );
        assert!(
            !dest_dir.join("source/.git").exists(),
            "snapshot extraction must not leave git metadata"
        );
    }

    /// Point git config discovery at empty sources and unset identity-related
    /// environment variables, restoring the previous environment on drop.
    struct IdentityEnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl IdentityEnvGuard {
        fn scrubbed() -> Self {
            const VARS: [&str; 5] = [
                "GIT_CONFIG_GLOBAL",
                "GIT_CONFIG_NOSYSTEM",
                "GIT_COMMITTER_NAME",
                "GIT_COMMITTER_EMAIL",
                "GIT_COMMITTER_DATE",
            ];
            let saved: Vec<_> = VARS
                .into_iter()
                .map(|var| (var, std::env::var_os(var)))
                .collect();
            // SAFETY: test-only; no other test in this crate reads these
            // variables to resolve a git identity (they set repo-local
            // identities instead), so transient mutation is harmless.
            unsafe {
                std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
                std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
                std::env::remove_var("GIT_COMMITTER_NAME");
                std::env::remove_var("GIT_COMMITTER_EMAIL");
                std::env::remove_var("GIT_COMMITTER_DATE");
            }
            Self { saved }
        }
    }

    impl Drop for IdentityEnvGuard {
        fn drop(&mut self) {
            // SAFETY: see IdentityEnvGuard::scrubbed.
            unsafe {
                for (var, value) in &self.saved {
                    match value {
                        Some(value) => std::env::set_var(var, value),
                        None => std::env::remove_var(var),
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn extract_git_metadata_clone_without_configured_identity() {
        let root = tempfile::tempdir().unwrap();
        let upstream = root.path().join("upstream");
        let mut upstream_repo = gix::init(&upstream).unwrap();
        {
            let mut config = upstream_repo.config_snapshot_mut();
            config
                .set_value(&gix::config::tree::User::NAME, "Wright Test")
                .unwrap();
            config
                .set_value(&gix::config::tree::User::EMAIL, "wright@example.invalid")
                .unwrap();
        }
        let blob_id = upstream_repo.write_blob(b"payload\n").unwrap().detach();
        let tree = gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "payload.txt".into(),
                oid: blob_id,
            }],
        };
        let tree_id = upstream_repo.write_object(&tree).unwrap().detach();
        let commit_id = upstream_repo
            .commit("HEAD", "initial", tree_id, Vec::<gix::ObjectId>::new())
            .unwrap();
        let commit_hex = commit_id.to_string();

        // A commit-hash ref forces the non-shallow mirror clone, which updates
        // refs/heads/* in the non-bare work repo and therefore writes reflogs.
        let manifest = PlanManifest::parse(&format!(
            r#"
name = "git-mirror-source"
version = "1.0.0"
release = 1
description = "test git mirror source"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "git"
url = "{upstream}"
ref = "{commit_hex}"
git_metadata = true
extract_to = "source"
"#,
            upstream = upstream.display()
        ))
        .unwrap();

        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("sources");
        let charge = Charge::new(&config, Arc::new(Semaphore::new(1)));
        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Under sudo there is no configured committer identity; the reflog
        // writes triggered by the mirror fetch must not abort the clone.
        let _env = IdentityEnvGuard::scrubbed();
        charge.extract(&manifest, &dest_dir).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.join("source/payload.txt")).unwrap(),
            "payload\n"
        );
        assert!(
            dest_dir.join("source/.git").exists(),
            "git_metadata sources keep their repository"
        );
    }

    fn test_charge(sources_dir: std::path::PathBuf) -> Charge {
        let mut config = GlobalConfig::default();
        config.general.source_dir = sources_dir;
        Charge::new(&config, Arc::new(Semaphore::new(1)))
    }

    fn test_manifest(sources_toml: &str) -> PlanManifest {
        PlanManifest::parse(&format!(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

{sources_toml}
"#
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn extract_places_local_source_at_original_basename() {
        let root = tempfile::tempdir().unwrap();
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        std::fs::write(sources_dir.join("demo-demo.service"), "unit file\n").unwrap();

        let manifest = test_manifest(
            r#"
[[sources]]
type = "local"
path = "demo.service"
"#,
        );
        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        test_charge(sources_dir)
            .extract(&manifest, &dest_dir)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.join("demo.service")).unwrap(),
            "unit file\n"
        );
        assert!(!dest_dir.join("demo-demo.service").exists());
    }

    #[tokio::test]
    async fn extract_places_http_source_at_url_basename() {
        let root = tempfile::tempdir().unwrap();
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        std::fs::write(sources_dir.join("demo-data.bin"), "payload").unwrap();

        let manifest = test_manifest(
            r#"
[[sources]]
type = "http"
sha256 = "SKIP"
url = "https://example.invalid/data.bin"
"#,
        );
        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        test_charge(sources_dir)
            .extract(&manifest, &dest_dir)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.join("data.bin")).unwrap(),
            "payload"
        );
    }

    #[tokio::test]
    async fn extract_renames_local_source_with_as() {
        let root = tempfile::tempdir().unwrap();
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        std::fs::write(sources_dir.join("renamed.conf"), "renamed\n").unwrap();

        let manifest = test_manifest(
            r#"
[[sources]]
type = "local"
path = "configs/app.conf"
as = "renamed.conf"
"#,
        );
        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        test_charge(sources_dir)
            .extract(&manifest, &dest_dir)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.join("renamed.conf")).unwrap(),
            "renamed\n"
        );
    }

    #[tokio::test]
    async fn extract_rejects_duplicate_workdir_destinations() {
        let root = tempfile::tempdir().unwrap();
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        std::fs::write(sources_dir.join("demo-app.json"), "{}").unwrap();

        let manifest = test_manifest(
            r#"
[[sources]]
type = "local"
path = "a/app.json"

[[sources]]
type = "local"
path = "b/app.json"
"#,
        );
        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let err = test_charge(sources_dir)
            .extract(&manifest, &dest_dir)
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("same work directory file"),
            "unexpected error: {err}"
        );
    }
}
