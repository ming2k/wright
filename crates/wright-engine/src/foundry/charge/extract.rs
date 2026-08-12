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
                    let repo = gix::init(&final_dest).map_err(|e| {
                        WrightError::ForgeError(format!("local git init failed: {e}"))
                    })?;
                    // An anonymous remote is enough: the fetch is driven by
                    // explicit refspecs, nothing is persisted in the repo config.
                    let remote = repo.remote_at(cache_str).map_err(|e| {
                        WrightError::ForgeError(format!("local git remote setup failed: {e}"))
                    })?;
                    let refspec_strings: Vec<String> = if uses_private_ref {
                        vec![format!("+{checkout_ref}:{checkout_ref}")]
                    } else {
                        // Mirror heads and tags so that branch names, tag names,
                        // and arbitrary commit hashes all resolve locally.
                        vec![
                            "+refs/heads/*:refs/heads/*".to_string(),
                            "+refs/tags/*:refs/tags/*".to_string(),
                        ]
                    };
                    let extra_refspecs: Vec<gix::refspec::RefSpec> = refspec_strings
                        .iter()
                        .map(|spec| {
                            gix::refspec::parse(
                                spec.as_str().into(),
                                gix::refspec::parse::Operation::Fetch,
                            )
                            .map(|r| r.to_owned())
                        })
                        .collect::<std::result::Result<Vec<_>, _>>()
                        .map_err(|e| {
                            WrightError::ForgeError(format!("invalid git refspec: {e}"))
                        })?;
                    let interrupt = std::sync::atomic::AtomicBool::new(false);
                    let connection =
                        remote.connect(gix::remote::Direction::Fetch).map_err(|e| {
                            WrightError::ForgeError(format!("local git connect failed: {e}"))
                        })?;
                    let prepare = connection
                        .prepare_fetch(
                            &mut gix::progress::Discard,
                            gix::remote::ref_map::Options {
                                extra_refspecs,
                                ..Default::default()
                            },
                        )
                        .map_err(|e| {
                            WrightError::ForgeError(format!("local git fetch failed: {e}"))
                        })?;
                    prepare
                        .receive(&mut gix::progress::Discard, &interrupt)
                        .map_err(|e| {
                            WrightError::ForgeError(format!("local git fetch failed: {e}"))
                        })?;
                    let id = repo.rev_parse_single(checkout_ref.as_str()).map_err(|e| {
                        WrightError::ForgeError(format!("failed to resolve ref {git_ref}: {e}"))
                    })?;
                    let tree = id
                        .object()
                        .map_err(|e| {
                            WrightError::ForgeError(format!("failed to load git object: {e}"))
                        })?
                        .peel_to_tree()
                        .map_err(|e| {
                            WrightError::ForgeError(format!("failed to load git tree: {e}"))
                        })?;
                    let mut index = repo.index_from_tree(&tree.id).map_err(|e| {
                        WrightError::ForgeError(format!("failed to build git index: {e}"))
                    })?;
                    let mut checkout_options = repo
                        .checkout_options(
                            gix::worktree::stack::state::attributes::Source::IdMapping,
                        )
                        .map_err(|e| {
                            WrightError::ForgeError(format!("git checkout setup failed: {e}"))
                        })?;
                    checkout_options.destination_is_initially_empty = true;
                    gix::worktree::state::checkout(
                        &mut index,
                        &final_dest,
                        repo.objects.clone().into_arc().map_err(|e| {
                            WrightError::ForgeError(format!("git object store error: {e}"))
                        })?,
                        &gix::progress::Discard,
                        &gix::progress::Discard,
                        &interrupt,
                        checkout_options,
                    )
                    .map_err(|e| WrightError::ForgeError(format!("git checkout failed: {e}")))?;
                    index.write(Default::default()).map_err(|e| {
                        WrightError::ForgeError(format!("failed to write git index: {e}"))
                    })?;
                    let head_target = match repo.try_find_reference(&checkout_ref) {
                        Ok(Some(reference)) => {
                            gix::refs::Target::Symbolic(reference.name().to_owned())
                        }
                        Ok(None) | Err(_) => gix::refs::Target::Object(id.detach()),
                    };
                    repo.edit_reference(gix::refs::transaction::RefEdit {
                        change: gix::refs::transaction::Change::Update {
                            log: gix::refs::transaction::LogChange::default(),
                            expected: gix::refs::transaction::PreviousValue::Any,
                            new: head_target,
                        },
                        name: "HEAD".try_into().expect("HEAD is a valid reference name"),
                        deref: false,
                    })
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
        let mut upstream_repo = gix::init(&upstream).unwrap();

        std::fs::write(upstream.join("payload.txt"), "from tagged source\n").unwrap();
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
        {
            let mut config = upstream_repo.config_snapshot_mut();
            config
                .set_value(&gix::config::tree::User::NAME, "Wright Test")
                .unwrap();
            config
                .set_value(&gix::config::tree::User::EMAIL, "wright@example.invalid")
                .unwrap();
        }
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

        let source_url = "https://example.invalid/upstream.git";
        let sources_dir = root.path().join("sources");
        let cache_path = sources_dir.join("git").join(git_cache_dir_name(source_url));
        std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        let cache_repo = gix::init_bare(&cache_path).unwrap();
        let remote = cache_repo.remote_at(upstream.to_str().unwrap()).unwrap();
        let refspec = gix::refspec::parse(
            "+refs/tags/*:refs/tags/*".into(),
            gix::refspec::parse::Operation::Fetch,
        )
        .unwrap()
        .to_owned();
        let interrupt = std::sync::atomic::AtomicBool::new(false);
        let connection = remote.connect(gix::remote::Direction::Fetch).unwrap();
        connection
            .prepare_fetch(
                &mut gix::progress::Discard,
                gix::remote::ref_map::Options {
                    extra_refspecs: vec![refspec],
                    ..Default::default()
                },
            )
            .unwrap()
            .receive(&mut gix::progress::Discard, &interrupt)
            .unwrap();
        let tag_id = cache_repo.rev_parse_single("refs/tags/v1.0.0").unwrap();
        cache_repo
            .reference(
                "refs/wright/v1.0.0",
                tag_id,
                gix::refs::transaction::PreviousValue::Any,
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
