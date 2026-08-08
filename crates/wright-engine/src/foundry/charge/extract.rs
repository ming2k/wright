use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::debug;

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::progress;
use wright_part::compression as compress;
use wright_part::store::sanitize_cache_filename;
use wright_plan::manifest::{PlanManifest, Source};

use super::git::{git_cache_dir_name, local_fetch_ref, uses_private_fetch_ref};
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
                    let git_dir_name = git_cache_dir_name(&processed_url);
                    let cache_path = self.cache_dir.join("git").join(&git_dir_name);
                    let git_ref = git
                        .r#ref
                        .as_deref()
                        .map(|r| variables::process_uri(r, manifest))
                        .unwrap_or_else(|| "HEAD".to_string());
                    let final_dest = if let Some(ref sub) = git.extract_to {
                        let sub = variables::process_uri(sub, manifest);
                        let p = dest_dir.join(&sub);
                        tokio::fs::create_dir_all(&p)
                            .await
                            .map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.join(&git_dir_name)
                    };
                    debug!(
                        "Extracting Git repo to {} (ref: {})...",
                        final_dest.display(),
                        git_ref
                    );
                    let cache_str = cache_path.to_str().ok_or_else(|| {
                        WrightError::ForgeError(format!(
                            "git cache path contains non-UTF-8 characters: {}",
                            cache_path.display()
                        ))
                    })?;
                    let uses_private_ref = uses_private_fetch_ref(&git_ref, git.depth);
                    let checkout_ref = if uses_private_ref {
                        local_fetch_ref(&git_ref)
                    } else {
                        git_ref.clone()
                    };
                    let repo = if uses_private_ref {
                        let repo = git2::Repository::init(&final_dest).map_err(|e| {
                            WrightError::ForgeError(format!("local git init failed: {e}"))
                        })?;
                        let mut remote = repo.remote("origin", cache_str).map_err(|e| {
                            WrightError::ForgeError(format!("local git remote setup failed: {e}"))
                        })?;
                        let refspec = format!("+{checkout_ref}:{checkout_ref}");
                        remote.fetch(&[refspec.as_str()], None, None).map_err(|e| {
                            WrightError::ForgeError(format!("local git fetch failed: {e}"))
                        })?;
                        drop(remote);
                        repo
                    } else {
                        git2::Repository::clone(cache_str, &final_dest).map_err(|e| {
                            WrightError::ForgeError(format!("local git clone failed: {e}"))
                        })?
                    };
                    let (object, reference) = repo
                        .revparse_ext(&checkout_ref)
                        .or_else(|_| repo.revparse_ext(&format!("origin/{git_ref}")))
                        .map_err(|e| {
                            WrightError::ForgeError(format!("failed to resolve ref {git_ref}: {e}"))
                        })?;
                    repo.checkout_tree(&object, None).map_err(|e| {
                        WrightError::ForgeError(format!("git checkout failed: {e}"))
                    })?;
                    match reference {
                        Some(gref) => {
                            let ref_name = gref.name().map_err(|error| {
                                WrightError::ForgeError(format!(
                                    "git reference name is invalid: {error}"
                                ))
                            })?;
                            repo.set_head(ref_name)
                        }
                        None => repo.set_head_detached(object.id()),
                    }
                    .map_err(|e| WrightError::ForgeError(format!("failed to update HEAD: {e}")))?;
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
                            WrightError::ForgeError(format!(
                                "failed to extract source {filename}: {e}"
                            ))
                        })?;
                    } else {
                        let dest_name = http
                            .r#as
                            .clone()
                            .unwrap_or_else(|| source_workdir_filename(&processed_url));
                        let dest = final_dest.join(&dest_name);
                        claim_workdir_dest(&mut placed, &dest)?;
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to copy non-archive source {dest_name} to work directory: {e}"
                            ))
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
                            WrightError::ForgeError(format!(
                                "failed to extract local source {filename}: {e}"
                            ))
                        })?;
                    } else {
                        let dest_name = local
                            .r#as
                            .clone()
                            .unwrap_or_else(|| source_workdir_filename(&processed_path));
                        let dest = final_dest.join(&dest_name);
                        claim_workdir_dest(&mut placed, &dest)?;
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to copy local source {dest_name} to work directory: {e}"
                            ))
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
    async fn extract_checks_out_private_shallow_git_ref() {
        let root = tempfile::tempdir().unwrap();
        let upstream = root.path().join("upstream");
        let upstream_repo = git2::Repository::init(&upstream).unwrap();
        let signature = git2::Signature::now("Wright Test", "wright@example.invalid").unwrap();

        std::fs::write(upstream.join("payload.txt"), "from tagged source\n").unwrap();
        let mut index = upstream_repo.index().unwrap();
        index.add_path(Path::new("payload.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = upstream_repo.find_tree(tree_id).unwrap();
        let commit_id = upstream_repo
            .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap();
        let commit = upstream_repo.find_commit(commit_id).unwrap();
        upstream_repo
            .tag("v1.0.0", commit.as_object(), &signature, "v1.0.0", false)
            .unwrap();

        let source_url = "https://example.invalid/upstream.git";
        let sources_dir = root.path().join("sources");
        let cache_path = sources_dir.join("git").join(git_cache_dir_name(source_url));
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let cache_repo = git2::Repository::init_bare(&cache_path).unwrap();
        let mut remote = cache_repo
            .remote("origin", upstream.to_str().unwrap())
            .unwrap();
        remote
            .fetch(&["+refs/tags/*:refs/tags/*"], None, None)
            .unwrap();
        drop(remote);
        let tag_id = cache_repo.revparse_single("refs/tags/v1.0.0").unwrap().id();
        cache_repo
            .reference(
                "refs/wright/v1.0.0",
                tag_id,
                true,
                "test private shallow ref",
            )
            .unwrap();
        drop(cache_repo);

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

        let mut config = GlobalConfig::default();
        config.general.source_dir = sources_dir;
        let charge = Charge::new(&config, Arc::new(Semaphore::new(1)));
        let dest_dir = root.path().join("work");
        std::fs::create_dir_all(&dest_dir).unwrap();

        charge.extract(&manifest, &dest_dir).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(dest_dir.join("source/payload.txt")).unwrap(),
            "from tagged source\n"
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
