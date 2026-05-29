use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::foundry::variables;
use crate::part::store::sanitize_cache_filename;
use crate::plan::manifest::{PlanManifest, Source};
use crate::util::{checksum, compress, download, progress};

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
            if let Ok(stored) = tokio::fs::read_to_string(&marker).await {
                if stored.trim() == fingerprint {
                    debug!(
                        event = "charge.cache_hit",
                        plan_name = %manifest.metadata.name,
                        "Source tree unchanged — reusing source/"
                    );
                    return Ok(ChargeResult { dir: source_dir, fingerprint });
                }
            }
            // Fingerprint mismatch — purge and rebuild.
            let _ = force_clean_source_dir(&source_dir).await;
        }

        // Don't start a fresh fetch/extract if the user already cancelled.
        if crate::isolation::reaper::is_cancelled() {
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

        Ok(ChargeResult { dir: source_dir, fingerprint })
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

    // ------------------------------------------------------------------
    // Fetch
    // ------------------------------------------------------------------

    async fn fetch(&self, manifest: &PlanManifest, plan_dir: &Path) -> Result<()> {
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
                let _permit = self
                    .network_pool
                    .acquire()
                    .await
                    .expect("network semaphore closed");
                let processed_url = variables::process_uri(&git.url, manifest);
                let git_dir_name = git_cache_dir_name(&processed_url);
                let git_cache_dir = self.cache_dir.join("git");
                if tokio::fs::metadata(&git_cache_dir).await.is_err() {
                    tokio::fs::create_dir_all(&git_cache_dir).await.ok();
                }
                let dest = git_cache_dir.join(&git_dir_name);
                let processed_ref = git.r#ref.as_deref().map(|r| variables::process_uri(r, manifest));
                let commit_id = self
                    .fetch_git_repo(
                        &processed_url,
                        processed_ref.as_deref(),
                        git.depth,
                        &dest,
                        &manifest.metadata.name,
                    )
                    .await?;
                debug!("Fetched Git commit: {} for {}", commit_id, git_dir_name);
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
                            warn!("Cached source {} hash mismatch, re-downloading...", filename);
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
                let filename =
                    source_cache_filename(&manifest.metadata.name, &processed_path);
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

    async fn fetch_git_repo(
        &self,
        git_url: &str,
        git_ref: Option<&str>,
        depth: Option<u32>,
        dest: &Path,
        scope: &str,
    ) -> Result<String> {
        let actual_ref = git_ref.unwrap_or("HEAD");
        let is_commit_hash =
            actual_ref.len() == 40 && actual_ref.chars().all(|c| c.is_ascii_hexdigit());
        let effective_depth = if is_commit_hash {
            tracing::debug!(
                "[{}] ref '{}' looks like a commit hash; disabling shallow clone",
                scope,
                actual_ref
            );
            None
        } else {
            depth
        };
        let label = progress::source_label(git_url);
        let is_fresh_clone = tokio::fs::metadata(dest).await.is_err();
        let repo = if is_fresh_clone {
            info!("[{}] Cloning Git repository: {}", scope, git_url);
            git2::Repository::init_bare(dest)
                .map_err(|e| WrightError::ForgeError(format!("git init failed: {e}")))?
        } else {
            git2::Repository::open_bare(dest)
                .map_err(|e| WrightError::ForgeError(format!("git open failed: {e}")))?
        };

        if !is_fresh_clone && let Ok(obj) = repo.revparse_single(actual_ref) {
            tracing::debug!("[{}] git ref '{}' already available locally; skipping fetch", scope, actual_ref);
            return Ok(obj.id().to_string());
        }

        let mut remote = repo
            .remote_anonymous(git_url)
            .map_err(|e| WrightError::ForgeError(format!("git remote setup failed: {e}")))?;
        let git_span = crate::cli_span!("Fetching", "{} ({})", label, scope);
        let span_for_cb = git_span.clone();
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.transfer_progress(move |stats| {
            let total_objects = stats.total_objects() as u64;
            if total_objects == 0 { return true; }
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
        fetch_opts.download_tags(git2::AutotagOption::All);
        if let Some(d) = effective_depth && d > 0 {
            fetch_opts.depth(d as i32);
        }
        remote.fetch(
            &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
            Some(&mut fetch_opts),
            None,
        ).map_err(|e| git_fetch_error(e, git_url, dest))?;
        drop(git_span);
        let obj = repo.revparse_single(actual_ref).map_err(|e| {
            WrightError::ForgeError(format!("failed to resolve git ref '{actual_ref}': {e}"))
        })?;
        Ok(obj.id().to_string())
    }

    // ------------------------------------------------------------------
    // Verify
    // ------------------------------------------------------------------

    async fn verify(&self, manifest: &PlanManifest) -> Result<()> {
        for (i, source) in manifest.sources.entries.iter().enumerate() {
            let http = match source {
                Source::Http(h) => h,
                _ => {
                    debug!("Skipping verification for non-HTTP source {}", i);
                    continue;
                }
            };
            if http.sha256 == "SKIP" {
                debug!("Skipping verification for HTTP source {} (SKIP)", i);
                continue;
            }
            let processed_url = variables::process_uri(&http.url, manifest);
            let filename = http
                .r#as
                .clone()
                .unwrap_or_else(|| source_cache_filename(&manifest.metadata.name, &processed_url));
            let path = self.cache_dir.join(&filename);
            if tokio::fs::metadata(&path).await.is_err() {
                return Err(WrightError::ValidationError(format!(
                    "source file missing: {filename}"
                )));
            }
            let actual_hash = checksum::sha256_file(&path)?;
            if actual_hash != http.sha256 {
                return Err(WrightError::ValidationError(format!(
                    "SHA256 mismatch for {filename}:\n  expected: {}\n  actual:   {}",
                    http.sha256, actual_hash
                )));
            }
            debug!("Verified source: {}", filename);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Extract
    // ------------------------------------------------------------------

    async fn extract(&self, manifest: &PlanManifest, dest_dir: &Path) -> Result<PathBuf> {
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
                        tokio::fs::create_dir_all(&p).await.map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.join(&git_dir_name)
                    };
                    debug!("Extracting Git repo to {} (ref: {})...", final_dest.display(), git_ref);
                    let cache_str = cache_path.to_str().ok_or_else(|| {
                        WrightError::ForgeError(format!(
                            "git cache path contains non-UTF-8 characters: {}",
                            cache_path.display()
                        ))
                    })?;
                    let repo = git2::Repository::clone(cache_str, &final_dest)
                        .map_err(|e| WrightError::ForgeError(format!("local git clone failed: {e}")))?;
                    let (object, reference) = repo
                        .revparse_ext(&git_ref)
                        .or_else(|_| repo.revparse_ext(&format!("origin/{git_ref}")))
                        .map_err(|e| WrightError::ForgeError(format!("failed to resolve ref {git_ref}: {e}")))?;
                    repo.checkout_tree(&object, None)
                        .map_err(|e| WrightError::ForgeError(format!("git checkout failed: {e}")))?;
                    match reference {
                        Some(gref) => {
                            let ref_name = gref.name().ok_or_else(|| {
                                WrightError::ForgeError("git reference name is non-UTF-8".to_string())
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
                        tokio::fs::create_dir_all(&p).await.map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.to_path_buf()
                    };
                    if is_part_file(&filename) {
                        let label = progress::source_label(&processed_url);
                        let _span = crate::cli_span!("Extracting", "{} ({})", label, manifest.metadata.name);
                        compress::extract_part(&cache_path, &final_dest).map_err(|e| {
                            WrightError::ForgeError(format!("failed to extract source {filename}: {e}"))
                        })?;
                    } else {
                        let dest = final_dest.join(&filename);
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to copy non-archive source {filename} to work directory: {e}"
                            ))
                        })?;
                    }
                }
                Source::Local(local) => {
                    let processed_path = variables::process_uri(&local.path, manifest);
                    let filename = source_cache_filename(&manifest.metadata.name, &processed_path);
                    let cache_path = self.cache_dir.join(&filename);
                    let final_dest = if let Some(ref sub) = local.extract_to {
                        let sub = variables::process_uri(sub, manifest);
                        let p = dest_dir.join(&sub);
                        tokio::fs::create_dir_all(&p).await.map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.to_path_buf()
                    };
                    if is_part_file(&filename) {
                        let label = progress::source_label(&processed_path);
                        let _span = crate::cli_span!("Extracting", "{} ({})", label, manifest.metadata.name);
                        compress::extract_part(&cache_path, &final_dest).map_err(|e| {
                            WrightError::ForgeError(format!("failed to extract local source {filename}: {e}"))
                        })?;
                    } else {
                        let dest = final_dest.join(&filename);
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to copy local source {filename} to work directory: {e}"
                            ))
                        })?;
                    }
                }
            }
        }
        Ok(dest_dir.to_path_buf())
    }

    // ------------------------------------------------------------------
    // Hash update utility
    // ------------------------------------------------------------------

    pub async fn update_hashes(&self, manifest: &PlanManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();
        if tokio::fs::metadata(&self.cache_dir).await.is_err() {
            tokio::fs::create_dir_all(&self.cache_dir).await.map_err(WrightError::IoError)?;
        }
        for source in manifest.sources.entries.iter() {
            match source {
                Source::Http(http) => {
                    let processed_url = variables::process_uri(&http.url, manifest);
                    let cache_filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_url)
                    });
                    let cache_path = self.cache_dir.join(&cache_filename);
                    if tokio::fs::metadata(&cache_path).await.is_ok() {
                        debug!("Using cached source: {}", cache_filename);
                    } else {
                        info!("Downloading {}...", processed_url);
                        download::download_file(
                            &processed_url,
                            &cache_path,
                            self.download_timeout,
                            &manifest.metadata.name,
                        )?;
                    }
                    let hash = checksum::sha256_file(&cache_path)?;
                    debug!("Computed hash: {}", hash);
                    new_hashes.push(hash);
                }
                Source::Git(_) | Source::Local(_) => {
                    new_hashes.push("SKIP".to_string());
                }
            }
        }
        if new_hashes.is_empty() {
            info!("No sources to update.");
            return Ok(());
        }
        let content = tokio::fs::read_to_string(manifest_path)
            .await
            .map_err(WrightError::IoError)?;
        let has_array_of_tables = content.contains("[[sources]]");
        let new_content = if has_array_of_tables {
            let sha256_re = regex::Regex::new(r#"(?m)^(sha256\s*=\s*)"[^"]*""#).unwrap();
            let mut result = content.clone();
            let mut hash_idx = 0;
            while let Some(m) = sha256_re.find(&result[..]) {
                if hash_idx < new_hashes.len() {
                    let replacement = format!(
                        "{}\"{}\"",
                        &result[m.start()..m.start() + result[m.start()..].find('"').unwrap()],
                        new_hashes[hash_idx]
                    );
                    result = format!("{}{}{}", &result[..m.start()], replacement, &result[m.end()..]);
                    hash_idx += 1;
                } else {
                    break;
                }
            }
            result
        } else {
            let re = regex::Regex::new(r"(?m)^sha256\s*=\s*\[[\s\S]*?\]").unwrap();
            let hashes_str = new_hashes
                .iter()
                .map(|h| format!("    \"{h}\""))
                .collect::<Vec<_>>()
                .join(",\n");
            let replacement = format!("sha256 = [\n{},\n]", hashes_str);
            if re.is_match(&content) {
                re.replace(&content, &replacement).to_string()
            } else {
                let uris_re = regex::Regex::new(r"(?m)^uris\s*=\s*\[[\s\S]*?\]").unwrap();
                if uris_re.is_match(&content) {
                    let uris_match = uris_re.find(&content).unwrap();
                    let mut c = content.clone();
                    c.insert_str(uris_match.end(), &format!("\n{replacement}"));
                    c
                } else {
                    return Err(WrightError::ForgeError(
                        "could not find sources or sha256 field in plan.toml".to_string(),
                    ));
                }
            }
        };
        tokio::fs::write(manifest_path, new_content)
            .await
            .map_err(WrightError::IoError)?;
        Ok(())
    }
}

// ------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------

fn source_cache_filename(part_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", part_name, basename))
}

/// Turn a libgit2 fetch failure into an actionable error.
///
/// When upstream history is rewritten (e.g. a force-push), the cached bare repo
/// still references objects the remote no longer advertises, so libgit2 aborts
/// the fetch with an ODB "object not found" error. The raw message is opaque, so
/// we explain the likely cause and point at the cache directory to remove.
fn git_fetch_error(e: git2::Error, url: &str, cache: &Path) -> WrightError {
    if e.class() == git2::ErrorClass::Odb {
        return WrightError::ForgeError(format!(
            "git fetch failed for {url}: {e}\n\
             The upstream history appears to have been rewritten (e.g. force-pushed), \
             so the cached clone references commits the remote no longer has.\n\
             Remove the stale cache and retry:\n    rm -rf {}",
            cache.display()
        ));
    }
    WrightError::ForgeError(format!("git fetch failed: {e}"))
}

fn git_cache_dir_name(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let last_segment = url.split('/').next_back().unwrap_or("repo");
    let stem = sanitize_cache_filename(last_segment.strip_suffix(".git").unwrap_or(last_segment));
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let hash = format!("{:x}", h.finalize());
    format!("{}-{}", stem, &hash[..8])
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

fn validate_local_path(plan_dir: &Path, relative_path: &str) -> Result<PathBuf> {
    let resolved = plan_dir.join(relative_path).canonicalize().map_err(|e| {
        WrightError::ValidationError(format!("local path not found: {relative_path} ({e})"))
    })?;
    let plan_abs = plan_dir.canonicalize().map_err(|e| {
        WrightError::ValidationError(format!("failed to resolve plan directory {}: {e}", plan_dir.display()))
    })?;
    if !resolved.starts_with(&plan_abs) {
        return Err(WrightError::ValidationError(format!(
            "local path escapes plan directory: {relative_path}"
        )));
    }
    Ok(resolved)
}

async fn force_clean_source_dir(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        tokio::fs::remove_dir_all(dir).await.map_err(|e| {
            WrightError::ForgeError(format!("failed to clean source dir {}: {}", dir.display(), e))
        })?;
    }
    Ok(())
}
