use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::debug;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_part::store::sanitize_cache_filename;
use wright_plan::manifest::{PlanManifest, Source};

mod extract;
mod fetch;
mod git;
mod update_hashes;
mod verify;

/// The built-in stages of source preparation, executed in order by
/// `Charge::prepare`.
pub const CHARGE_STAGES: &[&str] = &["fetch", "verify", "extract"];

/// Result of source preparation — a ready-to-build source tree.
pub struct ChargeResult {
    pub dir: PathBuf,
    pub fingerprint: String,
}

/// Prepares raw source materials for the forge.
///
/// The foundry metaphor: **Charge** is the act of loading raw ore into the
/// furnace — fetching, assaying, and breaking it down so the forge can work.
///
/// Charge owns the first three stages of a build: `fetch`, `verify`, `extract`.
/// These are built-in stages; they do not run user-defined scripts.
pub struct Charge {
    cache_dir: PathBuf,
    network_pool: Arc<Semaphore>,
    download_timeout: u64,
}

impl Charge {
    pub fn new(config: &GlobalConfig, network_pool: Arc<Semaphore>) -> Self {
        Self {
            cache_dir: config.general.source_dir.clone(),
            network_pool,
            download_timeout: config.network.download_timeout,
        }
    }

    /// The only public entry point. Idempotent.
    ///
    /// Flow: fetch → verify → extract → write `.charge_prepared` marker.
    /// If the marker's fingerprint matches, returns immediately.
    pub async fn prepare(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        build_root: &Path,
    ) -> Result<ChargeResult> {
        let source_dir = build_root.join("source");
        let marker = build_root.join(".charge_prepared");
        let fingerprint = self.fingerprint(manifest);

        if marker.exists() {
            if let Ok(stored) = tokio::fs::read_to_string(&marker).await
                && stored.trim() == fingerprint
            {
                debug!(
                    event = "charge.cache_hit",
                    plan_name = %manifest.metadata.name,
                    "Source tree unchanged — reusing source/"
                );
                return Ok(ChargeResult {
                    dir: source_dir,
                    fingerprint,
                });
            }
            // Fingerprint mismatch — purge and rebuild.
            let _ = force_clean_source_dir(&source_dir).await;
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
            .map_err(|e| WrightError::ForgeError(format!("failed to create source dir: {e}")))?;

        self.fetch(manifest, plan_dir).await?;
        self.verify(manifest).await?;
        self.extract(manifest, &source_dir).await?;

        tokio::fs::write(&marker, &fingerprint)
            .await
            .map_err(|e| WrightError::ForgeError(format!("failed to write charge marker: {e}")))?;

        Ok(ChargeResult {
            dir: source_dir,
            fingerprint,
        })
    }

    /// Compute a fingerprint of the manifest's sources section.
    pub fn fingerprint(&self, manifest: &PlanManifest) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for source in &manifest.sources.entries {
            match source {
                Source::Http(http) => {
                    hasher.update(b"http");
                    hasher.update(http.url.as_bytes());
                    hasher.update(http.sha256.as_bytes());
                    if let Some(ref r#as) = http.r#as {
                        hasher.update(r#as.as_bytes());
                    }
                    if let Some(ref ext) = http.extract_to {
                        hasher.update(ext.as_bytes());
                    }
                }
                Source::Git(git) => {
                    hasher.update(b"git");
                    hasher.update(git.url.as_bytes());
                    if let Some(ref r#ref) = git.r#ref {
                        hasher.update(r#ref.as_bytes());
                    }
                    if let Some(depth) = git.depth {
                        hasher.update(depth.to_le_bytes());
                    }
                    if let Some(ref ext) = git.extract_to {
                        hasher.update(ext.as_bytes());
                    }
                }
                Source::Local(local) => {
                    hasher.update(b"local");
                    hasher.update(local.path.as_bytes());
                    if let Some(ref ext) = local.extract_to {
                        hasher.update(ext.as_bytes());
                    }
                }
            }
        }
        format!("{:x}", hasher.finalize())
    }
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn source_cache_filename(part_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", part_name, basename))
}

async fn force_clean_source_dir(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        tokio::fs::remove_dir_all(dir).await.map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to clean source dir {}: {}",
                dir.display(),
                e
            ))
        })?;
    }
    Ok(())
}
