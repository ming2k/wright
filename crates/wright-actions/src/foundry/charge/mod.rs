use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::debug;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::foundry::variables;
use wright_part::store::sanitize_cache_filename;
use wright_plan::manifest::{PlanManifest, Source};

mod extract;
mod fetch;
mod git;
mod identity;
mod update_hashes;
mod verify;

pub use identity::{SourceDigest, source_provenance_lines};

/// The built-in stages of source preparation, executed in order by
/// `Charge::prepare`.
pub const CHARGE_STAGES: &[&str] = &["fetch", "verify", "extract"];

/// Result of source preparation — a ready-to-build source tree, carrying the
/// content address of the source set that produced it (ADR-0054).
pub struct ChargeResult {
    pub dir: PathBuf,
    /// Content address of the resolved sources. This is the sole source-derived
    /// input to the build key and the forge checkpoint chain, so it must travel
    /// with the prepared tree rather than being recomputed later.
    pub digest: SourceDigest,
}

/// Prepares raw source materials for the forge.
///
/// The foundry metaphor: **Charge** is the act of loading raw ore into the
/// furnace — fetching, assaying, and breaking it down so the forge can work.
///
/// Charge owns the first three stages of a build: `fetch`, `verify`, `extract`.
/// These are built-in stages; they do not run user-defined scripts.
#[derive(Clone)]
pub struct Charge {
    cache_dir: PathBuf,
    network_pool: Arc<Semaphore>,
    download_timeout: u64,
    /// Cold build (ADR-0057): never answer from an unproven cache entry and
    /// never short-circuit on the marker. The marker is still written, because
    /// it records what this invocation built rather than what it reused.
    no_cache: bool,
}

impl Charge {
    pub fn new(config: &GlobalConfig, network_pool: Arc<Semaphore>) -> Self {
        Self::with_no_cache(config, network_pool, false)
    }

    pub fn with_no_cache(
        config: &GlobalConfig,
        network_pool: Arc<Semaphore>,
        no_cache: bool,
    ) -> Self {
        Self {
            cache_dir: config.general.source_dir.clone(),
            network_pool,
            download_timeout: config.network.download_timeout,
            no_cache,
        }
    }

    /// The only public entry point. Idempotent.
    ///
    /// Flow: resolve source identity → (short-circuit) or fetch → verify →
    /// extract → write `.charge_prepared` marker.
    ///
    /// The marker stores the [`SourceDigest`] of the plan's *resolved* sources,
    /// not a rendering of its declarations. It is honoured only when the digest
    /// matches, the extracted tree is still present, and no source is volatile
    /// (ADR-0054) — so an edited local source can never be masked by a marker
    /// written for its predecessor (`[INV-SRC-05]`).
    ///
    /// The recorded digest is resolved *after* materialization: an unpinned
    /// source (`sha256 = "SKIP"`) has no address until its artefact exists, so
    /// the first build stores the real one and later builds short-circuit.
    pub async fn prepare(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        build_root: &Path,
    ) -> Result<ChargeResult> {
        let source_dir = build_root.join("source");
        let marker = build_root.join(".charge_prepared");
        let digest = self.resolve_sources(manifest, plan_dir)?;

        if !self.no_cache
            && !digest.is_volatile()
            && marker_matches(&marker, &digest).await
            && source_dir.is_dir()
        {
            debug!(
                event = "charge.cache_hit",
                plan_name = %manifest.metadata.name,
                source_digest = %digest.as_str(),
                "Source content unchanged — reusing source/"
            );
            return Ok(ChargeResult {
                dir: source_dir,
                digest,
            });
        }

        if digest.is_volatile() {
            debug!(
                event = "charge.volatile_sources",
                plan_name = %manifest.metadata.name,
                "Sources are not content-addressable — re-materializing source/"
            );
        }

        // Don't start a fresh fetch/extract if the user already cancelled.
        if crate::cancellation::is_cancelled() {
            return Err(WrightError::ForgeError("cancelled by user".into()));
        }

        // Ensure source directory exists and is clean.
        if tokio::fs::metadata(&source_dir).await.is_ok() {
            force_clean_source_dir(&source_dir).await?;
        }
        tokio::fs::create_dir_all(&source_dir)
            .await
            .map_err(|e| WrightError::context("failed to create source dir", e))?;
        crate::util::fs::ensure_public_dir_async(&source_dir)
            .await
            .ok();

        self.fetch(manifest, plan_dir).await?;
        self.verify(manifest).await?;
        self.extract(manifest, &source_dir).await?;

        // Re-resolve now that every source has bytes: an unpinned source is no
        // longer `unresolved`, so this build records an address that the next
        // build can match instead of re-materializing forever.
        let digest = self.resolve_sources(manifest, plan_dir)?;

        tokio::fs::write(&marker, digest.as_str())
            .await
            .map_err(|e| WrightError::context("failed to write charge marker", e))?;

        Ok(ChargeResult {
            dir: source_dir,
            digest,
        })
    }

    /// Total size of the plan's cached source archives — the download cost
    /// input for build-cost ledger records. Sources that are never cached
    /// (git clones carrying metadata) and cache misses contribute nothing.
    pub fn sources_bytes(&self, manifest: &PlanManifest) -> u64 {
        manifest
            .sources
            .entries
            .iter()
            .filter_map(|source| self.cache_path_for(manifest, source))
            .filter_map(|path| std::fs::metadata(&path).ok())
            .map(|meta| meta.len())
            .sum()
    }

    /// The cache file a source occupies (or would occupy) under
    /// `cache_dir`. `None` for sources that are never cached.
    pub(crate) fn cache_path_for(
        &self,
        manifest: &PlanManifest,
        source: &Source,
    ) -> Option<PathBuf> {
        source_cache_path(&self.cache_dir, manifest, source)
    }
}

/// The cache file a source occupies (or would occupy) under `cache_dir`.
///
/// Free function so purge planning can compute paths without constructing a
/// `Charge` (and without a network semaphore).
pub(crate) fn source_cache_path(
    cache_dir: &Path,
    manifest: &PlanManifest,
    source: &Source,
) -> Option<PathBuf> {
    let filename = match source {
        Source::Git(git) => {
            if git.git_metadata {
                // Cloned straight into the work directory at extract
                // time; nothing is cached.
                return None;
            }
            let processed_url = variables::process_uri(&git.url, manifest);
            let processed_ref = git
                .r#ref
                .as_deref()
                .map(|r| variables::process_uri(r, manifest))
                .unwrap_or_else(|| "HEAD".to_string());
            git::git_snapshot_filename(&processed_url, &processed_ref, git.submodules)
        }
        Source::Http(http) => {
            let processed_url = variables::process_uri(&http.url, manifest);
            http.r#as
                .clone()
                .unwrap_or_else(|| source_cache_filename(&manifest.metadata.name, &processed_url))
        }
        Source::Local(local) => {
            let processed_path = variables::process_uri(&local.path, manifest);
            local
                .r#as
                .clone()
                .unwrap_or_else(|| source_cache_filename(&manifest.metadata.name, &processed_path))
        }
    };
    Some(cache_dir.join(filename))
}

/// Cache entries whose content is provable: `http` sources with a concrete
/// `sha256` (not `SKIP`). These are inputs, not caches: a cold build keeps them
/// when their bytes still match the declaration (ADR-0057).
pub(crate) fn pinned_cache_entries(
    config: &GlobalConfig,
    manifest: &PlanManifest,
) -> Vec<(PathBuf, String)> {
    manifest
        .sources
        .entries
        .iter()
        .filter_map(|source| match source {
            Source::Http(http) if http.sha256 != "SKIP" => {
                source_cache_path(&config.general.source_dir, manifest, source)
                    .map(|path| (path, http.sha256.clone()))
            }
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

/// Whether a `.charge_prepared` marker records the same source digest.
async fn marker_matches(marker: &Path, digest: &SourceDigest) -> bool {
    match tokio::fs::read_to_string(marker).await {
        Ok(stored) => stored.trim() == digest.as_str(),
        Err(_) => false,
    }
}

fn source_cache_filename(part_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", part_name, basename))
}

async fn force_clean_source_dir(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        tokio::fs::remove_dir_all(dir).await.map_err(|e| {
            WrightError::context(format!("failed to clean source dir {}", dir.display()), e)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    fn config_in(root: &Path) -> GlobalConfig {
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.join("sources");
        config
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

    fn charge_in(root: &Path) -> Charge {
        Charge::new(&config_in(root), Arc::new(Semaphore::new(1)))
    }

    fn cold_charge_in(root: &Path) -> Charge {
        Charge::with_no_cache(&config_in(root), Arc::new(Semaphore::new(1)), true)
    }

    /// The incident ADR-0054 was written for, end to end: edit a local source,
    /// run charge again, and the prepared tree must carry the new bytes.
    #[tokio::test]
    async fn editing_a_local_source_re_materializes_the_tree() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        let unit = plan_dir.join("demo.service");
        std::fs::write(&unit, "ExecStart=/bin/old\n").unwrap();

        let charge = charge_in(root.path());
        let manifest = local_manifest("demo.service");
        let build_root = root.path().join("forge");

        charge
            .prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(build_root.join("source/demo.service")).unwrap(),
            "ExecStart=/bin/old\n"
        );

        std::fs::write(&unit, "ExecStart=/bin/new\n").unwrap();
        charge
            .prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(build_root.join("source/demo.service")).unwrap(),
            "ExecStart=/bin/new\n",
            "an edited local source must be re-materialized, not masked by the marker"
        );
    }

    /// The other half of the contract: unchanged content must still short-circuit,
    /// or the fix would be a rebuild-everything regression.
    #[tokio::test]
    async fn unchanged_local_source_reuses_the_prepared_tree() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(plan_dir.join("demo.service"), "ExecStart=/bin/true\n").unwrap();

        let charge = charge_in(root.path());
        let manifest = local_manifest("demo.service");
        let build_root = root.path().join("forge");

        let first = charge
            .prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();
        // Sabotage the extracted tree: a genuine cache hit must not re-extract
        // over it, so the sentinel survives.
        let sentinel = build_root.join("source/.sentinel");
        std::fs::write(&sentinel, "untouched").unwrap();

        let second = charge
            .prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();

        assert_eq!(first.digest.as_str(), second.digest.as_str());
        assert!(sentinel.exists(), "unchanged sources must reuse source/");
    }

    /// A cold build must not honour the marker even when it matches: the
    /// prepared tree is derived state, and `--no-cache` forbids reading it
    /// (ADR-0057).
    #[tokio::test]
    async fn cold_build_re_materializes_a_matching_marker() {
        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(plan_dir.join("demo.service"), "ExecStart=/bin/true\n").unwrap();

        let manifest = local_manifest("demo.service");
        let build_root = root.path().join("forge");
        let cold = cold_charge_in(root.path());

        cold.prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();
        let sentinel = build_root.join("source/.sentinel");
        std::fs::write(&sentinel, "stale").unwrap();

        cold.prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();

        assert!(
            !sentinel.exists(),
            "a cold build must replace the prepared tree instead of reusing it"
        );
        assert_eq!(
            std::fs::read_to_string(build_root.join("source/demo.service")).unwrap(),
            "ExecStart=/bin/true\n"
        );
    }

    /// An unpinned (`sha256 = "SKIP"`) source has no address until its artefact
    /// exists, so the first build must record the *post-fetch* address. If it
    /// recorded the pre-fetch placeholder, every later build would re-extract.
    #[tokio::test]
    async fn unpinned_http_source_converges_after_its_first_fetch() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let payload = b"payload-bytes".to_vec();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = payload.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    served.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&served);
            }
        });

        let root = tempfile::tempdir().unwrap();
        let plan_dir = root.path().join("plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        let manifest = PlanManifest::parse(&format!(
            r#"
name = "demo"
version = "1.0.0"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "http://127.0.0.1:{port}/data.bin"
sha256 = "SKIP"
"#
        ))
        .unwrap();

        let charge = charge_in(root.path());
        let build_root = root.path().join("forge");

        let first = charge
            .prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();
        assert!(
            !first.digest.is_volatile(),
            "after fetching, the artefact's bytes are the address"
        );

        // Second build: same address, so the tree is reused rather than rebuilt.
        let sentinel = build_root.join("source/.sentinel");
        std::fs::write(&sentinel, "untouched").unwrap();
        let second = charge
            .prepare(&manifest, &plan_dir, &build_root)
            .await
            .unwrap();

        assert_eq!(first.digest.as_str(), second.digest.as_str());
        assert!(
            sentinel.exists(),
            "an addressed SKIP source must short-circuit"
        );
    }
}
