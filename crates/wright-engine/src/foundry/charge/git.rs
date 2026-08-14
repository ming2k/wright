use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tracing::debug;

use crate::error::{Result, WrightError};
use crate::util::progress;
use wright_part::store::sanitize_cache_filename;

use super::Charge;

impl Charge {
    /// Fetch `git_ref` of `git_url` into a snapshot tarball at `dest`.
    ///
    /// The snapshot holds the checked-out tree of the pinned ref — no git
    /// metadata. Returns the resolved commit id, or `None` when the snapshot
    /// is already cached.
    ///
    /// Fetching is minimal-effort: a shallow fetch of exactly the requested
    /// ref. For 40-character commit hashes, servers that refuse shallow
    /// wants by hash fall back to a full mirror fetch.
    pub(super) fn fetch_git_snapshot(
        &self,
        git_url: &str,
        git_ref: Option<&str>,
        dest: &Path,
        scope: &str,
    ) -> Result<Option<String>> {
        let actual_ref = git_ref.unwrap_or("HEAD");
        if dest.exists() {
            debug!(
                "[{}] git snapshot already cached: {}",
                scope,
                dest.display()
            );
            return Ok(None);
        }

        let label = progress::source_label(git_url);
        // Stage the clone next to the cache so the final rename is atomic.
        let tmp = tempfile::tempdir_in(&self.cache_dir).map_err(WrightError::IoError)?;
        let repo = gix::init_bare(tmp.path().join("repo"))
            .map_err(|e| WrightError::context("git init failed", e))?;

        let resolve_target = if is_commit_hash(actual_ref) {
            let local_ref = local_fetch_ref(actual_ref);
            let shallow = fetch_refs(
                &repo,
                git_url,
                &[format!("+{actual_ref}:{local_ref}")],
                Some(1),
                gix::remote::fetch::Tags::None,
                &label,
                scope,
            );
            if let Err(e) = shallow {
                debug!(
                    "[{}] shallow fetch by commit hash failed ({}); falling back to full mirror",
                    scope, e
                );
                fetch_refs(
                    &repo,
                    git_url,
                    &mirror_refspecs(),
                    None,
                    gix::remote::fetch::Tags::All,
                    &label,
                    scope,
                )?;
            }
            actual_ref.to_string()
        } else {
            let local_ref = local_fetch_ref(actual_ref);
            fetch_refs(
                &repo,
                git_url,
                &[format!("+{actual_ref}:{local_ref}")],
                Some(1),
                gix::remote::fetch::Tags::None,
                &label,
                scope,
            )?;
            local_ref
        };

        let id = repo
            .rev_parse_single(resolve_target.as_str())
            .map_err(|e| {
                WrightError::context(format!("failed to resolve git ref '{actual_ref}'"), e)
            })?;
        let object = id
            .object()
            .map_err(|e| WrightError::context("failed to load git object", e))?;
        let commit_id = object
            .clone()
            .peel_to_commit()
            .map_err(|e| WrightError::context("failed to load git commit", e))?
            .id;
        let tree = object
            .peel_to_tree()
            .map_err(|e| WrightError::context("failed to load git tree", e))?;

        let snapshot_tmp = tmp.path().join("snapshot");
        write_tree_snapshot(&repo, tree.id, &snapshot_tmp)?;
        std::fs::rename(&snapshot_tmp, dest).map_err(WrightError::IoError)?;
        Ok(Some(commit_id.to_string()))
    }

    /// Clone `git_ref` of `git_url` into `dest` as a real working repository.
    ///
    /// Backs sources with `git_metadata = true`: the build runs git commands
    /// in the source tree (e.g. `git submodule update --init`). Such sources
    /// bypass the source cache and always fetch from upstream.
    pub(super) fn clone_git_source(
        &self,
        git_url: &str,
        git_ref: &str,
        dest: &Path,
        scope: &str,
    ) -> Result<()> {
        let label = progress::source_label(git_url);
        // Commit hashes cannot be fetched shallow everywhere, so they get a
        // full mirror; named refs are shallow-fetched into a private ref.
        let uses_private_ref = !is_commit_hash(git_ref);
        let checkout_ref = if uses_private_ref {
            local_fetch_ref(git_ref)
        } else {
            git_ref.to_string()
        };
        let mut repo =
            gix::init(dest).map_err(|e| WrightError::context("local git init failed", e))?;
        // A mirror fetch writes refs/heads/*, which triggers reflog writes in
        // the non-bare worktree repo. Reflog entries need a committer identity,
        // and there usually is none configured when running under sudo —
        // without a fallback the fetch aborts its ref transaction after the
        // pack was already received.
        repo.committer_or_set_generic_fallback()
            .map_err(|e| WrightError::context("git identity setup failed", e))?;
        // A mirror fetch writes refs/heads/*, which triggers reflog writes in
        // the non-bare worktree repo. Reflog entries need a committer identity,
        // and there usually is none configured when running under sudo —
        // without a fallback the fetch aborts its ref transaction after the
        // pack was already received.
        let (refspecs, tags, depth) = if uses_private_ref {
            (
                vec![format!("+{git_ref}:{checkout_ref}")],
                gix::remote::fetch::Tags::None,
                Some(1),
            )
        } else {
            // Mirror heads and tags so that branch names, tag names, and
            // arbitrary commit hashes all resolve locally.
            (mirror_refspecs(), gix::remote::fetch::Tags::All, None)
        };
        fetch_refs(&repo, git_url, &refspecs, depth, tags, &label, scope)?;

        let id = repo
            .rev_parse_single(checkout_ref.as_str())
            .map_err(|e| WrightError::context(format!("failed to resolve ref {git_ref}"), e))?;
        let tree = id
            .object()
            .map_err(|e| WrightError::context("failed to load git object", e))?
            .peel_to_tree()
            .map_err(|e| WrightError::context("failed to load git tree", e))?;
        let mut index = repo
            .index_from_tree(&tree.id)
            .map_err(|e| WrightError::context("failed to build git index", e))?;
        let mut checkout_options = repo
            .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
            .map_err(|e| WrightError::context("git checkout setup failed", e))?;
        checkout_options.destination_is_initially_empty = true;
        let interrupt = AtomicBool::new(false);
        gix::worktree::state::checkout(
            &mut index,
            dest,
            repo.objects
                .clone()
                .into_arc()
                .map_err(|e| WrightError::context("git object store error", e))?,
            &gix::progress::Discard,
            &gix::progress::Discard,
            &interrupt,
            checkout_options,
        )
        .map_err(|e| WrightError::context("git checkout failed", e))?;
        index
            .write(Default::default())
            .map_err(|e| WrightError::context("failed to write git index", e))?;
        let head_target = match repo.try_find_reference(&checkout_ref) {
            Ok(Some(reference)) => gix::refs::Target::Symbolic(reference.name().to_owned()),
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
        .map_err(|e| WrightError::context("failed to update HEAD", e))?;
        Ok(())
    }
}

/// Fetch the given refspecs from `git_url` into `repo`.
fn fetch_refs(
    repo: &gix::Repository,
    git_url: &str,
    refspec_strings: &[String],
    depth: Option<u32>,
    tags: gix::remote::fetch::Tags,
    label: &str,
    scope: &str,
) -> Result<()> {
    // An anonymous remote is enough: the fetch is driven by explicit
    // refspecs, nothing is persisted in the repo config.
    let remote = repo
        .remote_at(git_url)
        .map_err(|e| WrightError::context("git remote setup failed", e))?;
    let remote = remote.with_fetch_tags(tags);
    let extra_refspecs: Vec<gix::refspec::RefSpec> = refspec_strings
        .iter()
        .map(|spec| {
            gix::refspec::parse(spec.as_str().into(), gix::refspec::parse::Operation::Fetch)
                .map(|r| r.to_owned())
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| WrightError::context("invalid git refspec", e))?;

    let git_span = crate::cli_span!("Fetching", "{} ({})", label, scope);
    let mut fetch_progress = FetchProgress::new(git_span.clone());
    let interrupt = AtomicBool::new(false);
    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(|e| WrightError::context("git fetch failed", e))?;
    let mut prepare = connection
        .prepare_fetch(
            &mut fetch_progress,
            gix::remote::ref_map::Options {
                extra_refspecs,
                ..Default::default()
            },
        )
        .map_err(|e| WrightError::context("git fetch failed", e))?;
    if let Some(d) = depth
        && d > 0
        && let Some(depth) = NonZeroU32::new(d)
    {
        prepare = prepare.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(depth));
    }
    let fetch_result = prepare.receive(&mut fetch_progress, &interrupt);
    drop(git_span);
    fetch_result.map_err(|e| WrightError::context(format!("git fetch failed for {git_url}"), e))?;
    Ok(())
}

/// Write a git tree as a `.tar.zst` snapshot: plain file entries only, no git
/// metadata. Submodule pins (gitlinks) carry no content and are skipped.
/// Entry order follows the tree and mtimes are zeroed, so the same commit
/// always produces the same snapshot.
fn write_tree_snapshot(repo: &gix::Repository, tree: gix::ObjectId, out: &Path) -> Result<()> {
    let file = std::fs::File::create(out).map_err(WrightError::IoError)?;
    let encoder = zstd::Encoder::new(file, 3)
        .map_err(|e| WrightError::context("zstd encoder init failed", e))?;
    let mut builder = tar::Builder::new(encoder);
    write_tree_entries(repo, tree, Path::new(""), &mut builder)?;
    let encoder = builder
        .into_inner()
        .map_err(|e| WrightError::context("tar finish failed", e))?;
    encoder
        .finish()
        .map_err(|e| WrightError::context("zstd finish failed", e))?;
    Ok(())
}

fn write_tree_entries(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    prefix: &Path,
    builder: &mut tar::Builder<impl std::io::Write>,
) -> Result<()> {
    use gix::objs::tree::EntryKind;

    let tree = repo
        .find_tree(tree)
        .map_err(|e| WrightError::context("failed to load git tree", e))?;
    for entry in tree.iter() {
        let entry = entry.map_err(|e| WrightError::context("failed to decode git tree", e))?;
        let path = prefix.join(os_str_from_bytes(entry.filename()));
        match entry.mode().kind() {
            EntryKind::Blob | EntryKind::BlobExecutable => {
                let blob = repo
                    .find_blob(entry.oid().to_owned())
                    .map_err(|e| WrightError::context("failed to load git blob", e))?;
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(if entry.mode().kind() == EntryKind::BlobExecutable {
                    0o755
                } else {
                    0o644
                });
                header.set_size(blob.data.len() as u64);
                header.set_mtime(0);
                header
                    .set_path(&path)
                    .map_err(|e| WrightError::context("tar set path failed", e))?;
                header.set_cksum();
                builder
                    .append(&header, &blob.data[..])
                    .map_err(|e| WrightError::context("tar append failed", e))?;
            }
            EntryKind::Link => {
                let target = repo
                    .find_blob(entry.oid().to_owned())
                    .map_err(|e| WrightError::context("failed to load git blob", e))?;
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_mode(0o777);
                header.set_size(0);
                header.set_mtime(0);
                builder
                    .append_link(
                        &mut header,
                        &path,
                        os_str_from_bytes(target.data.as_slice()),
                    )
                    .map_err(|e| WrightError::context("tar append link failed", e))?;
            }
            EntryKind::Tree => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(0o755);
                header.set_size(0);
                header.set_mtime(0);
                header
                    .set_path(&path)
                    .map_err(|e| WrightError::context("tar set path failed", e))?;
                header.set_cksum();
                builder
                    .append(&header, std::io::empty())
                    .map_err(|e| WrightError::context("tar append dir failed", e))?;
                write_tree_entries(repo, entry.oid().to_owned(), &path, builder)?;
            }
            // A gitlink only records another repository's commit — there is
            // no content to snapshot.
            EntryKind::Commit => {}
        }
    }
    Ok(())
}

#[cfg(unix)]
fn os_str_from_bytes(bytes: &[u8]) -> &std::ffi::OsStr {
    std::os::unix::ffi::OsStrExt::from_bytes(bytes)
}

fn mirror_refspecs() -> Vec<String> {
    vec![
        "+refs/heads/*:refs/heads/*".to_string(),
        "+refs/tags/*:refs/tags/*".to_string(),
    ]
}

/// Filename of the snapshot tarball caching one pinned git source tree,
/// derived from the repository URL and the requested ref.
pub(super) fn git_snapshot_filename(git_url: &str, git_ref: &str) -> String {
    use sha2::{Digest, Sha256};
    let last_segment = git_url.split('/').next_back().unwrap_or("repo");
    let stem = sanitize_cache_filename(last_segment.strip_suffix(".git").unwrap_or(last_segment));
    let mut h = Sha256::new();
    h.update(git_url.as_bytes());
    let hash = format!("{:x}", h.finalize());
    format!(
        "{}-{}-{}.tar.zst",
        stem,
        sanitize_ref_component(git_ref),
        &hash[..8]
    )
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

/// Map a requested git ref into a private, path-safe ref namespace.
///
/// Fetches store the single ref they request here instead of mirroring
/// upstream's `refs/heads/*` and `refs/tags/*`. Encoding the ref keeps
/// different refs of the same repo from colliding.
pub(super) fn local_fetch_ref(git_ref: &str) -> String {
    format!("refs/wright/{}", sanitize_ref_component(git_ref))
}

fn sanitize_ref_component(git_ref: &str) -> String {
    git_ref
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
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

    /// Read a snapshot tarball back into (path, contents) pairs.
    fn read_snapshot(path: &Path) -> Vec<(String, String)> {
        let file = std::fs::File::open(path).unwrap();
        let decoder = zstd::Decoder::new(file).unwrap();
        let mut archive = tar::Archive::new(decoder);
        let mut entries = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.header().entry_type() != tar::EntryType::Regular {
                continue;
            }
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mut contents = String::new();
            std::io::Read::read_to_string(&mut entry, &mut contents).unwrap();
            entries.push((path, contents));
        }
        entries
    }

    #[test]
    fn snapshot_fetch_creates_tarball_for_tag_ref() {
        let root = tempfile::tempdir().unwrap();
        let (upstream, _branch, first, _tip) = make_upstream(root.path());
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        let charge = test_charge(sources_dir.clone());

        let url = upstream.to_str().unwrap();
        let dest = sources_dir.join(git_snapshot_filename(url, "v1.0.0"));
        let commit = charge
            .fetch_git_snapshot(url, Some("v1.0.0"), &dest, "test")
            .unwrap()
            .expect("fresh fetch reports the commit");
        assert_eq!(commit, first);

        let entries = read_snapshot(&dest);
        assert_eq!(
            entries,
            vec![("a.txt".to_string(), "one\n".to_string())],
            "snapshot should hold exactly the tagged tree"
        );
    }

    #[test]
    fn snapshot_fetch_falls_back_to_full_mirror_for_commit_hash() {
        let root = tempfile::tempdir().unwrap();
        let (upstream, _branch, first, _tip) = make_upstream(root.path());
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        let charge = test_charge(sources_dir.clone());

        let url = upstream.to_str().unwrap();
        let dest = sources_dir.join(git_snapshot_filename(url, &first));
        let commit = charge
            .fetch_git_snapshot(url, Some(&first), &dest, "test")
            .unwrap()
            .expect("fresh fetch reports the commit");
        assert_eq!(commit, first);

        let entries = read_snapshot(&dest);
        assert_eq!(
            entries,
            vec![("a.txt".to_string(), "one\n".to_string())],
            "snapshot should hold exactly the pinned tree"
        );
    }

    #[test]
    fn snapshot_fetch_skips_existing_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let (upstream, _branch, _first, _tip) = make_upstream(root.path());
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        let charge = test_charge(sources_dir.clone());

        let url = upstream.to_str().unwrap();
        let dest = sources_dir.join(git_snapshot_filename(url, "v1.0.0"));
        charge
            .fetch_git_snapshot(url, Some("v1.0.0"), &dest, "test")
            .unwrap();

        // The upstream is gone; a cached snapshot must still be usable.
        std::fs::remove_dir_all(&upstream).unwrap();
        let commit = charge
            .fetch_git_snapshot(url, Some("v1.0.0"), &dest, "test")
            .unwrap();
        assert!(commit.is_none(), "cached snapshot skips the fetch");
    }

    #[test]
    fn clone_git_source_checks_out_worktree() {
        let root = tempfile::tempdir().unwrap();
        let (upstream, _branch, _first, _tip) = make_upstream(root.path());
        let sources_dir = root.path().join("sources");
        std::fs::create_dir_all(&sources_dir).unwrap();
        let charge = test_charge(sources_dir);

        let dest = root.path().join("work");
        std::fs::create_dir_all(&dest).unwrap();
        charge
            .clone_git_source(upstream.to_str().unwrap(), "v1.0.0", &dest, "test")
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(dest.join(".git").exists(), "clone keeps git metadata");
    }
}
