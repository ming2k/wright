//! Content-addressed identity for a plan's resolved sources (ADR-0054).
//!
//! Everything that stands between a plan file and the bytes it deploys — the
//! charge marker, the build key, the forge stage hash chain — must be keyed on
//! the *resolved content* of the plan's sources, never on the plan's
//! declaration of them. [`SourceDigest`] is that key.
//!
//! Before this module, a `type = "local"` source was hashed as the string
//! `local.path`. Editing the file changed no cache key in the system, so
//! `wright upgrade` re-ran its whole ceremony and redeployed the previous
//! artefact. The digest closes that class of defect by construction: a byte
//! change in any local source changes the digest, and the digest is the only
//! source-derived input to every downstream key (`[INV-SRC-01]`).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::debug;

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::checksum;
use wright_plan::manifest::{PlanManifest, Source};

use super::Charge;
use super::git::git_snapshot_filename;

/// Format version of the digest record.
///
/// Bumping this string invalidates every `.charge_prepared` marker and every
/// checkpoint written under the previous algorithm. That is the point: an
/// identity function that changes must not be allowed to match the artefacts
/// of the old one.
pub(crate) const DIGEST_FORMAT_VERSION: &str = "wright-source-identity-v1";

/// The content address of a plan's fully resolved source set.
///
/// Two properties travel together:
///
/// * the hex digest, which seeds every cache key, and
/// * volatility, which says whether the digest is *trustworthy* — i.e. whether
///   every source's content was knowable without materializing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDigest {
    hex: String,
    volatile: bool,
}

impl SourceDigest {
    /// The hex digest, as stored in `.charge_prepared` and hashed into the
    /// build key.
    pub fn as_str(&self) -> &str {
        &self.hex
    }

    /// True when at least one source's content cannot be known until it has
    /// been fetched or cloned (`[INV-SRC-04]`).
    ///
    /// A volatile plan is exempt from every cache: charge always
    /// re-materializes, the forge never honours a checkpoint, and the CAS is
    /// neither consulted nor populated. A plan whose inputs are not
    /// content-addressable gets no reuse — that is the honest answer, and the
    /// only one that cannot go stale.
    pub fn is_volatile(&self) -> bool {
        self.volatile
    }
}

impl std::fmt::Display for SourceDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex)
    }
}

/// One source's content address, plus whether resolving it required work.
struct SourceAddress {
    /// Length-delimited field list describing the source and its content.
    record: String,
    volatile: bool,
}

impl Charge {
    /// Resolve every source to its content address and hash the set (ADR-0054).
    ///
    /// Reading local files is the only filesystem work performed here; no
    /// network access and no materialization happens, so this is cheap enough
    /// to run before every build, every CAS lookup, and every pre-build
    /// short-circuit check.
    pub fn resolve_sources(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
    ) -> Result<SourceDigest> {
        // Plan-relative sources resolve against the directory the plan was
        // loaded from. The explicit `plan_dir` is the caller's declaration of
        // that directory; the manifest wins when it carries one, so build
        // identity and audit provenance can never hash different directories.
        let plan_dir = manifest.plan_dir.as_deref().unwrap_or(plan_dir);

        // The format version is the digest's first field: a marker written by a
        // previous identity function can never match one written by this one.
        let mut fields = vec![
            DIGEST_FORMAT_VERSION.to_string(),
            manifest.sources.entries.len().to_string(),
        ];

        let mut volatile = false;
        for source in &manifest.sources.entries {
            let address = self.source_address(manifest, plan_dir, source)?;
            volatile |= address.volatile;
            fields.push(address.record);
        }

        Ok(SourceDigest {
            hex: hash_fields(&fields),
            volatile,
        })
    }

    /// Content address of a single source entry.
    fn source_address(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        source: &Source,
    ) -> Result<SourceAddress> {
        Ok(match source {
            Source::Local(local) => {
                let processed = variables::process_uri(&local.path, manifest);
                let content = local_content_address(Some(plan_dir), &processed)?;
                debug!(
                    event = "charge.local_source_addressed",
                    path = %processed,
                    sha256 = %content,
                    "Local source content-addressed"
                );
                SourceAddress {
                    record: hash_fields(&[
                        "local",
                        &processed,
                        &content,
                        local.r#as.as_deref().unwrap_or(""),
                        local.extract_to.as_deref().unwrap_or(""),
                    ]),
                    volatile: false,
                }
            }
            Source::Http(http) => {
                let processed = variables::process_uri(&http.url, manifest);
                let (address, volatile) = if http.sha256 == "SKIP" {
                    // Unpinned: the address is the cached artefact's bytes.
                    // Until that artefact exists there is nothing to key on, so
                    // the source is volatile and the plan is exempt from caches.
                    match self
                        .cache_path_for(manifest, source)
                        .filter(|path| path.exists())
                        .map(|path| checksum::sha256_file(&path))
                    {
                        Some(Ok(hash)) => (hash, false),
                        _ => ("unresolved".to_string(), true),
                    }
                } else {
                    (http.sha256.clone(), false)
                };
                SourceAddress {
                    record: hash_fields(&[
                        "http",
                        &processed,
                        &address,
                        http.r#as.as_deref().unwrap_or(""),
                        http.extract_to.as_deref().unwrap_or(""),
                    ]),
                    volatile,
                }
            }
            Source::Git(git) => {
                let processed_url = variables::process_uri(&git.url, manifest);
                let processed_ref = git
                    .r#ref
                    .as_deref()
                    .map(|r| variables::process_uri(r, manifest))
                    .unwrap_or_else(|| "HEAD".to_string());
                let submodules = git.submodules.to_string();
                let extract_to = git.extract_to.as_deref().unwrap_or("");

                if git.git_metadata {
                    // Bypasses the source cache and always clones from upstream
                    // (ADR-0038), so its content is unknowable without doing
                    // the work.
                    SourceAddress {
                        record: hash_fields(&[
                            "git-metadata",
                            &processed_url,
                            &processed_ref,
                            &submodules,
                            extract_to,
                        ]),
                        volatile: true,
                    }
                } else {
                    // A snapshot-cached ref is an explicit pin: one immutable
                    // snapshot per ref, refreshed by clearing the source cache.
                    // Binding the record to the snapshot filename ties the
                    // identity to the artefact that will actually be extracted.
                    let snapshot =
                        git_snapshot_filename(&processed_url, &processed_ref, git.submodules);
                    SourceAddress {
                        record: hash_fields(&[
                            "git",
                            &processed_url,
                            &processed_ref,
                            &submodules,
                            &snapshot,
                            extract_to,
                        ]),
                        volatile: false,
                    }
                }
            }
        })
    }
}

/// Hash a field list with length prefixes, so that no two distinct sets of
/// fields can produce the same byte stream.
fn hash_fields<S: AsRef<str>>(fields: &[S]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        let field = field.as_ref();
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Resolve a `type = "local"` source path against its plan directory.
///
/// The path must exist and must not escape the plan directory: a plan is not
/// allowed to pull arbitrary host files into a build.
pub(super) fn resolve_local_path(plan_dir: &Path, relative_path: &str) -> Result<PathBuf> {
    let resolved = plan_dir.join(relative_path).canonicalize().map_err(|e| {
        WrightError::ValidationError(format!("local path not found: {relative_path} ({e})"))
    })?;
    let plan_abs = plan_dir.canonicalize().map_err(|e| {
        WrightError::ValidationError(format!(
            "failed to resolve plan directory {}: {e}",
            plan_dir.display()
        ))
    })?;
    if !resolved.starts_with(&plan_abs) {
        return Err(WrightError::ValidationError(format!(
            "local path escapes plan directory: {relative_path}"
        )));
    }
    Ok(resolved)
}

/// One audit line per source, recording the content address that participated
/// in the build (ADR-0023 `source_checksums`, ADR-0055).
///
/// The line is the *declaration plus its content address*: `http <url>
/// sha256=<sum>`, `git <url> ref=<ref>`, `local <path> sha256=<file sum>`.
///
/// This is what makes "the plan changed" answerable for a plan whose only
/// change is the bytes of a file bundled beside `plan.toml`: `plan_checksum`
/// covers `plan.toml` alone, so a ledger built only from it cannot see the
/// file.
pub fn source_provenance_lines(manifest: &PlanManifest) -> Result<Vec<String>> {
    manifest
        .sources
        .entries
        .iter()
        .map(|source| match source {
            Source::Http(http) => Ok(format!(
                "http {} sha256={}",
                variables::process_uri(&http.url, manifest),
                http.sha256
            )),
            Source::Git(git) => Ok(format!(
                "git {} ref={}",
                variables::process_uri(&git.url, manifest),
                git.r#ref
                    .as_deref()
                    .map(|r| variables::process_uri(r, manifest))
                    .unwrap_or_else(|| "HEAD".to_string())
            )),
            Source::Local(local) => {
                let processed = variables::process_uri(&local.path, manifest);
                let address = local_content_address(manifest.plan_dir.as_deref(), &processed)?;
                Ok(format!("local {processed} sha256={address}"))
            }
        })
        .collect()
}

/// Content address of a `local` source: the SHA-256 of its file bytes.
///
/// Shared by build identity and audit provenance so the two can never disagree
/// about what a plan's sources hash to.
///
/// `[INV-SRC-02]` the address is the bytes and nothing else — never the path,
/// size, or mtime.  `[INV-SRC-06]` an unresolvable or unreadable local source is
/// an error, never a silently degraded address.
fn local_content_address(plan_dir: Option<&Path>, processed_path: &str) -> Result<String> {
    let plan_dir = plan_dir.ok_or_else(|| {
        WrightError::ValidationError(format!(
            "cannot resolve local source '{processed_path}': \
             the manifest was not loaded from a plan directory"
        ))
    })?;
    let file = resolve_local_path(plan_dir, processed_path)?;
    let metadata = std::fs::metadata(&file).map_err(|e| {
        WrightError::ValidationError(format!(
            "local source '{processed_path}' could not be read ({}): {e}",
            file.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(WrightError::ValidationError(format!(
            "local source '{processed_path}' is not a regular file ({}); \
             directories cannot be copied into the work directory",
            file.display()
        )));
    }
    checksum::sha256_file(&file).map_err(|e| {
        WrightError::context(format!("failed to hash local source {processed_path}"), e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalConfig;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    fn charge_in(root: &Path) -> Charge {
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.join("sources");
        Charge::new(&config, Arc::new(Semaphore::new(1)))
    }

    fn local_manifest(path: &str) -> PlanManifest {
        PlanManifest::parse(&format!(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "local"
path = "{path}"
"#
        ))
        .expect("valid manifest")
    }

    #[test]
    fn editing_a_local_source_changes_its_digest() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        let unit = plan_dir.join("demo.service");
        std::fs::write(&unit, "ExecStart=/bin/old\n").unwrap();

        let charge = charge_in(root.path());
        let manifest = local_manifest("demo.service");
        let before = charge.resolve_sources(&manifest, &plan_dir).unwrap();

        // Same path, same length, different bytes: the incident's exact shape.
        std::fs::write(&unit, "ExecStart=/bin/new\n").unwrap();
        let after = charge.resolve_sources(&manifest, &plan_dir).unwrap();

        assert_ne!(
            before.as_str(),
            after.as_str(),
            "content change must change the digest"
        );
    }

    #[test]
    fn identical_local_content_yields_identical_digest() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(plan_dir.join("demo.service"), "ExecStart=/bin/true\n").unwrap();

        let charge = charge_in(root.path());
        let manifest = local_manifest("demo.service");
        let first = charge.resolve_sources(&manifest, &plan_dir).unwrap();
        let second = charge.resolve_sources(&manifest, &plan_dir).unwrap();

        assert_eq!(first.as_str(), second.as_str());
        assert!(!first.is_volatile());
    }

    #[test]
    fn pinned_http_source_is_addressed_by_its_declared_checksum() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();

        let manifest = PlanManifest::parse(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.invalid/demo.tar.gz"
sha256 = "abc123"
"#,
        )
        .unwrap();

        let digest = charge_in(root.path())
            .resolve_sources(&manifest, &plan_dir)
            .unwrap();
        assert!(
            !digest.is_volatile(),
            "a pinned source is content-addressed"
        );
    }

    #[test]
    fn unpinned_http_source_is_volatile_until_cached() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();

        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("sources");
        let charge = Charge::new(&config, Arc::new(Semaphore::new(1)));

        let manifest = PlanManifest::parse(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.invalid/demo.tar.gz"
sha256 = "SKIP"
"#,
        )
        .unwrap();

        let before = charge.resolve_sources(&manifest, &plan_dir).unwrap();
        assert!(before.is_volatile(), "an unfetched SKIP source is volatile");

        // Materialize the cache artefact as a fetch would.
        std::fs::create_dir_all(&config.general.source_dir).unwrap();
        let cached = charge
            .cache_path_for(&manifest, &manifest.sources.entries[0])
            .unwrap();
        std::fs::write(&cached, b"payload").unwrap();

        let after = charge.resolve_sources(&manifest, &plan_dir).unwrap();
        assert!(
            !after.is_volatile(),
            "a fetched SKIP source is addressed by its bytes"
        );
        assert_ne!(before.as_str(), after.as_str());
    }

    #[test]
    fn git_metadata_sources_are_volatile() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();

        let manifest = PlanManifest::parse(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "git"
url = "https://example.invalid/demo.git"
ref = "main"
git_metadata = true
"#,
        )
        .unwrap();

        let digest = charge_in(root.path())
            .resolve_sources(&manifest, &plan_dir)
            .unwrap();
        assert!(
            digest.is_volatile(),
            "a source that always clones from upstream cannot be content-addressed"
        );
    }

    #[test]
    fn local_path_escaping_the_plan_directory_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(root.path().join("outside"), "secret").unwrap();

        let charge = charge_in(root.path());
        let manifest = local_manifest("../outside");
        let err = charge.resolve_sources(&manifest, &plan_dir).unwrap_err();
        assert!(
            err.to_string().contains("escapes plan directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn directory_local_source_is_rejected_with_a_clear_error() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(plan_dir.join("bundle")).unwrap();

        let charge = charge_in(root.path());
        let manifest = local_manifest("bundle");
        let err = charge.resolve_sources(&manifest, &plan_dir).unwrap_err();
        assert!(
            err.to_string().contains("not a regular file"),
            "unexpected error: {err}"
        );
    }
}
