use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
                    // A non-spurious fetch failure against an existing cache
                    // usually means the shallow cache could not be updated
                    // incrementally. This is usually NOT an upstream problem
                    // (a moved side-branch or a shallow-boundary mismatch is
                    // enough); refreshing the cache resolves it cleanly.
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
            match gix::init_bare(dest) {
                Ok(r) => r,
                Err(e) => {
                    return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                        "git init failed: {e}"
                    )));
                }
            }
        } else {
            match gix::open(dest) {
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
        // shallow negotiation, which is the dominant source of spurious
        // negotiation failures. A full (non-shallow) fetch has no shallow
        // boundary, so mirroring is safe there and keeps arbitrary commit hashes
        // resolvable.
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
            && let Ok(id) = repo.rev_parse_single(resolve_target.as_str())
        {
            tracing::debug!(
                "[{}] git ref '{}' already available locally; skipping fetch",
                scope,
                actual_ref
            );
            return GitFetchAttempt::Done(id.to_string());
        }

        // An anonymous remote is sufficient: gix negotiates the fetch purely
        // from the explicit refspecs passed below, so nothing needs to be
        // persisted in the repository configuration.
        let remote = match repo.remote_at(git_url) {
            Ok(r) => r,
            Err(e) => {
                return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                    "git remote setup failed: {e}"
                )));
            }
        };
        // A shallow fetch wants only the requested ref, so don't pull every tag
        // (which would re-introduce the broad negotiation we are avoiding). A
        // full fetch mirrors everything and therefore mirrors all tags as well.
        let remote = remote.with_fetch_tags(if shallow {
            gix::remote::fetch::Tags::None
        } else {
            gix::remote::fetch::Tags::All
        });

        let extra_refspecs: Vec<gix::refspec::RefSpec> = match refspecs
            .iter()
            .map(|spec| {
                gix::refspec::parse(spec.as_str().into(), gix::refspec::parse::Operation::Fetch)
                    .map(|r| r.to_owned())
            })
            .collect::<std::result::Result<Vec<_>, _>>()
        {
            Ok(specs) => specs,
            Err(e) => {
                return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                    "invalid git refspec: {e}"
                )));
            }
        };

        let git_span = crate::cli_span!("Fetching", "{} ({})", label, scope);
        let mut fetch_progress = FetchProgress::new(git_span.clone());
        let interrupt = AtomicBool::new(false);

        // Connection and handshake failures survive a cache refresh, so they
        // are terminal right away.
        let connection = match remote.connect(gix::remote::Direction::Fetch) {
            Ok(c) => c,
            Err(e) => {
                return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                    "git fetch failed: {e}"
                )));
            }
        };
        let mut prepare = match connection.prepare_fetch(
            &mut fetch_progress,
            gix::remote::ref_map::Options {
                extra_refspecs,
                ..Default::default()
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                return GitFetchAttempt::Failed(WrightError::ForgeError(format!(
                    "git fetch failed: {e}"
                )));
            }
        };
        if let Some(d) = effective_depth
            && d > 0
            && let Some(depth) = NonZeroU32::new(d)
        {
            prepare = prepare.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(depth));
        }
        let fetch_result = prepare.receive(&mut fetch_progress, &interrupt);
        drop(git_span);

        match fetch_result {
            Ok(_outcome) => {}
            Err(e) if is_stale_cache_candidate(&e) => {
                return GitFetchAttempt::StaleCache;
            }
            Err(e) => {
                return GitFetchAttempt::Failed(git_fetch_error(e, git_url, dest));
            }
        }

        match repo.rev_parse_single(resolve_target.as_str()) {
            Ok(id) => GitFetchAttempt::Done(id.to_string()),
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

/// A stale cache is only worth refreshing for non-spurious receive-phase
/// failures (negotiation, pack transfer or resolution, local ref/object
/// updates). Missing remote refs, transport-level failures, and transient
/// network errors all survive a cache refresh, so they are reported
/// immediately instead.
fn is_stale_cache_candidate(e: &gix::remote::fetch::Error) -> bool {
    use gix::protocol::transport::IsSpuriousError;
    !e.is_spurious()
        && !matches!(
            e,
            gix::remote::fetch::Error::NoMapping { .. } | gix::remote::fetch::Error::Client(_)
        )
}

/// Turn a fetch failure into an actionable error.
fn git_fetch_error(e: gix::remote::fetch::Error, url: &str, cache: &Path) -> WrightError {
    if is_stale_cache_candidate(&e) {
        return WrightError::ForgeError(format!(
            "git fetch failed for {url}: {e}\n\
             The shallow cache could not be refreshed automatically.\n\
             Remove the cache manually and retry:\n    rm -rf {}",
            cache.display()
        ));
    }
    WrightError::ForgeError(format!("git fetch failed: {e}"))
}

/// prodash progress adapter that forwards fetch step counts to the wright
/// span-based progress display. Children (receive, resolve, …) share the
/// parent span but track their own counters, matching the sequential phases
/// of a fetch.
#[derive(Clone)]
struct FetchProgress {
    span: tracing::Span,
    step: Arc<AtomicUsize>,
    max: Arc<AtomicUsize>,
}

impl FetchProgress {
    fn new(span: tracing::Span) -> Self {
        Self {
            span,
            step: Arc::new(AtomicUsize::new(0)),
            max: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn record(&self) {
        progress::record_bytes(
            &self.span,
            self.step.load(Ordering::Relaxed) as u64,
            self.max.load(Ordering::Relaxed) as u64,
        );
    }
}

impl gix::progress::Count for FetchProgress {
    fn set(&self, step: gix::progress::Step) {
        self.step.store(step, Ordering::Relaxed);
        self.record();
    }

    fn step(&self) -> gix::progress::Step {
        self.step.load(Ordering::Relaxed)
    }

    fn inc_by(&self, step: gix::progress::Step) {
        self.step.fetch_add(step, Ordering::Relaxed);
        self.record();
    }

    fn counter(&self) -> gix::progress::StepShared {
        self.step.clone()
    }
}

impl gix::progress::Progress for FetchProgress {
    fn init(&mut self, max: Option<gix::progress::Step>, _unit: Option<gix::progress::Unit>) {
        self.max.store(max.unwrap_or(0), Ordering::Relaxed);
        self.record();
    }

    fn set_max(&mut self, max: Option<gix::progress::Step>) -> Option<gix::progress::Step> {
        let previous = self.max.swap(max.unwrap_or(0), Ordering::Relaxed);
        (previous > 0).then_some(previous)
    }

    fn set_name(&mut self, _name: String) {}

    fn name(&self) -> Option<String> {
        None
    }

    fn id(&self) -> gix::progress::Id {
        gix::progress::UNKNOWN
    }

    fn message(&self, _level: gix::progress::MessageLevel, _message: String) {}
}

impl gix::progress::NestedProgress for FetchProgress {
    type SubProgress = Self;

    fn add_child(&mut self, _name: impl Into<String>) -> Self {
        FetchProgress::new(self.span.clone())
    }

    fn add_child_with_id(&mut self, _name: impl Into<String>, _id: gix::progress::Id) -> Self {
        FetchProgress::new(self.span.clone())
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Semaphore;

    use super::*;
    use crate::config::GlobalConfig;

    /// Create an upstream repository with two commits on its default branch
    /// and a `v1.0.0` tag pointing at the first commit. Returns the upstream
    /// path, the default branch name, and the two commit ids (oldest first).
    fn make_upstream(root: &Path) -> (std::path::PathBuf, String, String, String) {
        let upstream = root.join("upstream");
        let mut repo = gix::init(&upstream).unwrap();
        {
            let mut config = repo.config_snapshot_mut();
            config
                .set_value(&gix::config::tree::User::NAME, "Wright Test")
                .unwrap();
            config
                .set_value(&gix::config::tree::User::EMAIL, "wright@example.invalid")
                .unwrap();
        }
        let mut parents = Vec::<gix::ObjectId>::new();
        let mut commit_ids = Vec::new();
        for (name, contents) in [("a.txt", "one\n"), ("b.txt", "two\n")] {
            let blob_id = repo.write_blob(contents.as_bytes()).unwrap().detach();
            let tree = gix::objs::Tree {
                entries: vec![gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: name.into(),
                    oid: blob_id,
                }],
            };
            let tree_id = repo.write_object(&tree).unwrap().detach();
            let commit_id = repo
                .commit("HEAD", format!("add {name}"), tree_id, parents.clone())
                .unwrap();
            parents = vec![commit_id.detach()];
            commit_ids.push(commit_id.to_string());
        }
        let branch = repo
            .head_name()
            .unwrap()
            .expect("born")
            .shorten()
            .to_string();
        repo.reference(
            "refs/tags/v1.0.0",
            gix::ObjectId::from_hex(commit_ids[0].as_bytes()).unwrap(),
            gix::refs::transaction::PreviousValue::Any,
            "test tag",
        )
        .unwrap();
        (upstream, branch, commit_ids.remove(0), commit_ids.remove(0))
    }

    fn test_charge(sources_dir: std::path::PathBuf) -> Charge {
        let mut config = GlobalConfig::default();
        config.general.source_dir = sources_dir;
        Charge::new(&config, Arc::new(Semaphore::new(1)))
    }

    #[tokio::test]
    async fn fetch_git_repo_full_fetch_mirrors_and_skips_when_current() {
        let root = tempfile::tempdir().unwrap();
        let (upstream, branch, _first, tip) = make_upstream(root.path());
        let cache = root.path().join("cache");
        let charge = test_charge(root.path().join("sources"));

        let url = upstream.to_str().unwrap();
        let id = charge
            .fetch_git_repo(url, Some(&branch), None, &cache, "test")
            .await
            .unwrap();
        assert_eq!(id, tip, "full fetch should resolve the branch tip");

        // A second fetch against the populated cache resolves the same commit
        // without contacting the remote again.
        let id = charge
            .fetch_git_repo(url, Some(&branch), None, &cache, "test")
            .await
            .unwrap();
        assert_eq!(id, tip);

        // Mirrored tags resolve as well.
        let id = charge
            .fetch_git_repo(url, Some("v1.0.0"), None, &cache, "test")
            .await
            .unwrap();
        assert_eq!(id, _first);
    }

    #[tokio::test]
    async fn fetch_git_repo_shallow_fetch_uses_private_ref() {
        let root = tempfile::tempdir().unwrap();
        let (upstream, _branch, first, _tip) = make_upstream(root.path());
        let cache = root.path().join("cache-shallow");
        let charge = test_charge(root.path().join("sources"));

        let url = upstream.to_str().unwrap();
        let id = charge
            .fetch_git_repo(url, Some("v1.0.0"), Some(1), &cache, "test")
            .await
            .unwrap();
        assert_eq!(id, first);

        // The shallow fetch stores the ref in the private namespace and does
        // not mirror public heads.
        let repo = gix::open(&cache).unwrap();
        assert!(
            repo.try_find_reference("refs/wright/v1.0.0")
                .unwrap()
                .is_some(),
            "shallow fetch should create the private ref"
        );
        assert!(
            repo.try_find_reference("refs/tags/v1.0.0")
                .unwrap()
                .is_none(),
            "shallow fetch should not mirror tags"
        );

        // Re-fetching the same shallow ref resolves from the cache.
        let id = charge
            .fetch_git_repo(url, Some("v1.0.0"), Some(1), &cache, "test")
            .await
            .unwrap();
        assert_eq!(id, first);
    }
}
