use std::path::Path;
use tracing::{debug, info};

use crate::error::{Result, WrightError};
use crate::util::progress;
use wright_part::store::sanitize_cache_filename;

use super::Charge;

impl Charge {
    pub(super) async fn fetch_git_repo(
        &self,
        git_url: &str,
        git_ref: Option<&str>,
        depth: Option<u32>,
        dest: &Path,
        scope: &str,
    ) -> Result<String> {
        let actual_ref = git_ref.unwrap_or("HEAD");
        let effective_depth = effective_git_depth(actual_ref, depth);
        if depth.is_some() && effective_depth.is_none() && is_commit_hash(actual_ref) {
            tracing::debug!(
                "[{}] ref '{}' looks like a commit hash; disabling shallow clone",
                scope,
                actual_ref
            );
        }
        let label = progress::source_label(git_url);

        let mut retry_stale = false;
        loop {
            let is_fresh_clone = tokio::fs::metadata(dest).await.is_err();

            let attempt = self.git_fetch_attempt(
                git_url,
                actual_ref,
                effective_depth,
                dest,
                scope,
                &label,
                is_fresh_clone,
                retry_stale,
            );

            match attempt {
                GitFetchAttempt::Done(id) => return Ok(id),
                GitFetchAttempt::StaleCache if !retry_stale => {
                    // An ODB error here means the shallow cache could not be
                    // updated incrementally. This is usually NOT an upstream
                    // problem (a moved side-branch or a shallow-boundary mismatch
                    // is enough); refreshing the cache resolves it cleanly.
                    debug!(
                        "[{}] shallow git cache could not be updated incrementally; \
                         refreshing cache: {}",
                        scope,
                        dest.display()
                    );
                    tokio::fs::remove_dir_all(dest).await.map_err(|rm_err| {
                        WrightError::ForgeError(format!(
                            "failed to remove stale git cache {}: {rm_err}",
                            dest.display()
                        ))
                    })?;
                    retry_stale = true;
                    continue;
                }
                GitFetchAttempt::StaleCache => {
                    return Err(WrightError::ForgeError(format!(
                        "git fetch failed for {git_url}: refreshing the shallow cache did not \
                         resolve the issue.\n\
                         Remove the cache manually and retry:\n    rm -rf {}",
                        dest.display()
                    )));
                }
                GitFetchAttempt::Failed(err) => return Err(err),
            }
        }
    }

    fn git_fetch_attempt(
        &self,
        git_url: &str,
        actual_ref: &str,
        effective_depth: Option<u32>,
        dest: &Path,
        scope: &str,
        label: &str,
        is_fresh_clone: bool,
        force_fetch: bool,
    ) -> GitFetchAttempt {
        let repo = if is_fresh_clone {
            info!("[{}] Cloning Git repository: {}", scope, git_url);
            match git2::Repository::init_bare(dest) {
                Ok(r) => r,
                Err(e) => {
                    return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                        "git init failed: {e}"
                    )));
                }
            }
        } else {
            match git2::Repository::open_bare(dest) {
                Ok(r) => r,
                Err(e) => {
                    return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                        "git open failed: {e}"
                    )));
                }
            }
        };

        // Decide what to fetch. For shallow fetches, only request the single ref
        // we actually need, stored in a private namespace. Mirroring every branch
        // and tag (`+refs/heads/*` / `+refs/tags/*`) drags unrelated upstream
        // branches — which active repos routinely rebase or force-push — into the
        // shallow negotiation. libgit2 then aborts with an ODB error even though
        // the ref we build is untouched, which we previously misread as upstream
        // history rewrites. A full (non-shallow) fetch has no shallow boundary, so
        // mirroring is safe there and keeps arbitrary commit hashes resolvable.
        let shallow = matches!(effective_depth, Some(d) if d > 0);
        let local_ref = local_fetch_ref(actual_ref);
        let (refspecs, resolve_target): (Vec<String>, String) = if shallow {
            (vec![format!("+{actual_ref}:{local_ref}")], local_ref)
        } else {
            (
                vec![
                    "+refs/heads/*:refs/heads/*".to_string(),
                    "+refs/tags/*:refs/tags/*".to_string(),
                ],
                actual_ref.to_string(),
            )
        };

        if !is_fresh_clone
            && !force_fetch
            && let Ok(obj) = repo.revparse_single(&resolve_target)
        {
            tracing::debug!(
                "[{}] git ref '{}' already available locally; skipping fetch",
                scope,
                actual_ref
            );
            return GitFetchAttempt::Done(obj.id().to_string());
        }

        // Use a named remote so that libgit2 can persist fetch configuration
        // (url + refspec) in the repo.  Anonymous remotes lack this state,
        // which breaks shallow-fetch negotiation on incremental updates
        // (libgit2/libgit2#1430).
        let mut remote = match repo.find_remote("origin") {
            Ok(r) => r,
            Err(_) => match repo.remote("origin", git_url) {
                Ok(r) => r,
                Err(e) => {
                    return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                        "git remote setup failed: {e}"
                    )));
                }
            },
        };
        let git_span = crate::cli_span!("Fetching", "{} ({})", label, scope);
        let span_for_cb = git_span.clone();
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.transfer_progress(move |stats| {
            let total_objects = stats.total_objects() as u64;
            if total_objects == 0 {
                return true;
            }
            let received = stats.received_objects() as u64;
            let indexed = stats.indexed_objects() as u64;
            let total_deltas = stats.total_deltas() as u64;
            let indexed_deltas = stats.indexed_deltas() as u64;
            let (position, length) = if received < total_objects {
                (received, total_objects)
            } else if indexed < total_objects {
                (indexed, total_objects)
            } else if total_deltas > 0 && indexed_deltas < total_deltas {
                (indexed_deltas, total_deltas)
            } else {
                (total_objects, total_objects)
            };
            progress::record_bytes(&span_for_cb, position, length);
            true
        });
        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);
        // A shallow fetch wants only the requested ref, so don't pull every tag
        // (which would re-introduce the broad negotiation we are avoiding). A full
        // fetch mirrors everything and benefits from autotagging.
        fetch_opts.download_tags(if shallow {
            git2::AutotagOption::None
        } else {
            git2::AutotagOption::All
        });
        if let Some(d) = effective_depth
            && d > 0
        {
            fetch_opts.depth(d as i32);
        }
        let refspec_refs: Vec<&str> = refspecs.iter().map(String::as_str).collect();
        let fetch_result = remote.fetch(&refspec_refs, Some(&mut fetch_opts), None);
        drop(git_span);

        match fetch_result {
            Ok(()) => {}
            Err(e) if e.class() == git2::ErrorClass::Odb => {
                return GitFetchAttempt::StaleCache;
            }
            Err(e) => {
                return GitFetchAttempt::Failed(git_fetch_error(e, git_url, dest));
            }
        }

        drop(remote);
        match repo.revparse_single(&resolve_target) {
            Ok(obj) => GitFetchAttempt::Done(obj.id().to_string()),
            Err(e) => GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                "failed to resolve git ref '{actual_ref}': {e}"
            ))),
        }
    }
}

enum GitFetchAttempt {
    Done(String),
    StaleCache,
    Failed(WrightError),
}

/// Turn a libgit2 fetch failure into an actionable error.
///
/// ODB-class failures are handled upstream as a shallow-cache refresh (see
/// [`GitFetchAttempt::StaleCache`]); by the time we reach here the error is some
/// other failure (network, auth, missing ref, …), so report it verbatim.
fn git_fetch_error(e: git2::Error, url: &str, cache: &Path) -> WrightError {
    if e.class() == git2::ErrorClass::Odb {
        return WrightError::ForgeError(format!(
            "git fetch failed for {url}: {e}\n\
             The shallow cache could not be refreshed automatically.\n\
             Remove the cache manually and retry:\n    rm -rf {}",
            cache.display()
        ));
    }
    WrightError::ForgeError(format!("git fetch failed: {e}"))
}

fn is_commit_hash(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.chars().all(|c| c.is_ascii_hexdigit())
}

fn effective_git_depth(git_ref: &str, depth: Option<u32>) -> Option<u32> {
    if is_commit_hash(git_ref) { None } else { depth }
}

pub(super) fn uses_private_fetch_ref(git_ref: &str, depth: Option<u32>) -> bool {
    matches!(effective_git_depth(git_ref, depth), Some(d) if d > 0)
}

/// Map a requested git ref into a private, path-safe ref namespace.
///
/// Shallow fetches store the single ref they request here instead of mirroring
/// upstream's `refs/heads/*` and `refs/tags/*`. Encoding the ref keeps different
/// refs of the same cached repo from colliding.
pub(super) fn local_fetch_ref(git_ref: &str) -> String {
    let safe: String = git_ref
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("refs/wright/{safe}")
}

pub(super) fn git_cache_dir_name(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let last_segment = url.split('/').next_back().unwrap_or("repo");
    let stem = sanitize_cache_filename(last_segment.strip_suffix(".git").unwrap_or(last_segment));
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let hash = format!("{:x}", h.finalize());
    format!("{}-{}", stem, &hash[..8])
}
