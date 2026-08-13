use std::path::{Path, PathBuf};
use tracing::{debug, warn};

use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::util::{checksum, download, progress};
use wright_plan::manifest::{PlanManifest, Source};

use super::git::git_snapshot_filename;
use super::{Charge, source_cache_filename};

impl Charge {
    // ------------------------------------------------------------------
    // Fetch
    // ------------------------------------------------------------------

    pub(super) async fn fetch(&self, manifest: &PlanManifest, plan_dir: &Path) -> Result<()> {
        if tokio::fs::metadata(&self.cache_dir).await.is_err() {
            tokio::fs::create_dir_all(&self.cache_dir)
                .await
                .map_err(WrightError::IoError)?;
        }

        let futs = manifest
            .sources
            .entries
            .iter()
            .map(|source| self.fetch_one(manifest, plan_dir, source));
        futures_util::future::try_join_all(futs).await?;
        Ok(())
    }

    async fn fetch_one(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        source: &Source,
    ) -> Result<()> {
        match source {
            Source::Git(git) => {
                let processed_url = variables::process_uri(&git.url, manifest);
                let processed_ref = git
                    .r#ref
                    .as_deref()
                    .map(|r| variables::process_uri(r, manifest))
                    .unwrap_or_else(|| "HEAD".to_string());
                if git.git_metadata {
                    // Cloned straight into the work directory at extract
                    // time; nothing is cached.
                    return Ok(());
                }
                let filename = git_snapshot_filename(&processed_url, &processed_ref);
                let dest = self.cache_dir.join(&filename);
                if tokio::fs::metadata(&dest).await.is_err() {
                    let _permit = self
                        .network_pool
                        .acquire()
                        .await
                        .expect("network semaphore closed");
                    if let Some(commit_id) = self.fetch_git_snapshot(
                        &processed_url,
                        Some(processed_ref.as_str()),
                        &dest,
                        &manifest.metadata.name,
                    )? {
                        debug!("Fetched Git commit: {} for {}", commit_id, filename);
                    }
                }
            }
            Source::Http(http) => {
                let processed_url = variables::process_uri(&http.url, manifest);
                let filename = http.r#as.clone().unwrap_or_else(|| {
                    source_cache_filename(&manifest.metadata.name, &processed_url)
                });
                let dest = self.cache_dir.join(&filename);
                let skip_verify = http.sha256 == "SKIP";
                let mut needs_download = true;

                if tokio::fs::metadata(&dest).await.is_ok() {
                    if skip_verify {
                        debug!("Source {} already cached (SKIP verification)", filename);
                        needs_download = false;
                    } else if let Ok(actual_hash) = checksum::sha256_file(&dest) {
                        if actual_hash == http.sha256 {
                            debug!("Source {} already cached and verified", filename);
                            needs_download = false;
                        } else {
                            warn!(
                                "Cached source {} hash mismatch, re-downloading...",
                                filename
                            );
                            let _ = tokio::fs::remove_file(&dest).await;
                        }
                    }
                }

                if needs_download {
                    let _permit = self
                        .network_pool
                        .acquire()
                        .await
                        .expect("network semaphore closed");
                    let url = processed_url.clone();
                    let dest_owned = dest.clone();
                    let timeout = self.download_timeout;
                    let scope = manifest.metadata.name.clone();
                    tokio::task::spawn_blocking(move || {
                        download::download_file(&url, &dest_owned, timeout, &scope)
                    })
                    .await
                    .map_err(|e| WrightError::ForgeError(format!("download join: {e}")))??;
                    if !skip_verify {
                        let actual_hash = checksum::sha256_file(&dest)?;
                        if actual_hash != http.sha256 {
                            return Err(WrightError::ValidationError(format!(
                                "Downloaded file {} failed verification!\n  Expected: {}\n  Actual:   {}",
                                filename, http.sha256, actual_hash
                            )));
                        }
                    }
                }
            }
            Source::Local(local) => {
                let processed_path = variables::process_uri(&local.path, manifest);
                let local_path = validate_local_path(plan_dir, &processed_path)?;
                let filename = local.r#as.clone().unwrap_or_else(|| {
                    source_cache_filename(&manifest.metadata.name, &processed_path)
                });
                let dest = self.cache_dir.join(&filename);
                let label = progress::source_label(&processed_path);
                let _span = crate::cli_span!("Fetching", "{} ({})", label, manifest.metadata.name);
                tokio::fs::copy(&local_path, &dest).await.map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to copy local file {} to cache: {}",
                        local_path.display(),
                        e
                    ))
                })?;
            }
        }
        Ok(())
    }
}

fn validate_local_path(plan_dir: &Path, relative_path: &str) -> Result<PathBuf> {
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
