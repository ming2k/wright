pub mod checkpoint;
pub mod executor;
pub mod layers;
pub mod logging;
pub mod mvp;
pub mod pipeline;
pub mod variables;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::forge::layers::force_clean_dir;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::isolation::ResourceLimits;
use crate::part::store::sanitize_cache_filename;
use crate::plan::manifest::{OutputConfig, PlanManifest, Source};
use crate::util::{checksum, compress, download, progress};

pub struct ForgeResult {
    pub output_dir: PathBuf,
    pub work_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub split_part_dirs: std::collections::HashMap<String, PathBuf>,
}

pub struct Forger {
    config: GlobalConfig,
    executors: executor::ExecutorRegistry,
    /// Counted semaphore that caps concurrent source downloads across
    /// the whole process. Sized by `config.network.max_concurrent_downloads`.
    network_pool: Arc<Semaphore>,
}

fn source_cache_filename(part_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", part_name, basename))
}

/// CAS (Content-Addressable Storage) filename for the global source cache.
///
/// When `sha256` is known and non-SKIP, the name is
/// `[first_12_hex_chars]-[sanitized_filename]` — e.g.
/// `a51897bf1d2e-nginx-1.25.3.tar.gz`.
///
/// When `sha256` is unavailable or SKIP, falls back to the part-name-prefixed
/// scheme.
#[allow(dead_code)]
fn source_cache_cas_filename(part_name: &str, uri: &str, sha256: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    let clean = sanitize_cache_filename(basename);
    if sha256 != "SKIP" && !sha256.is_empty() {
        let prefix = &sha256[..12.min(sha256.len())];
        format!("{}-{}", prefix, clean)
    } else {
        // Fallback: prefix with part name for uniqueness.
        sanitize_cache_filename(&format!("{}-{}", part_name, basename))
    }
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

async fn ensure_clean_dir(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        match tokio::fs::remove_dir_all(dir).await {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                // A stale overlayfs mount may linger at <dir>/target from a
                // previous run.  Lazy-unmount it and retry the removal.
                let _ = nix::mount::umount2(&dir.join("target"), nix::mount::MntFlags::MNT_DETACH);
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if let Err(e2) = tokio::fs::remove_dir_all(dir).await {
                    warn!(event = "cleanup.directory_failed", path = %dir.display(), error = %e2, reason = "stale_overlayfs", "could not clean {} (busy after retry); a process or mount may still hold it", dir.display());
                }
            }
            Err(e) => {
                warn!(event = "cleanup.directory_failed", path = %dir.display(), error = %e, "could not clean {}: {}", dir.display(), e);
            }
        }
    }
    tokio::fs::create_dir_all(dir).await.map_err(|e| {
        WrightError::ForgeError(format!(
            "failed to create forge directory {}: {}",
            dir.display(),
            e
        ))
    })
}

fn validate_local_path(plan_dir: &Path, relative_path: &str) -> Result<PathBuf> {
    let resolved = plan_dir.join(relative_path).canonicalize().map_err(|e| {
        WrightError::ValidationError(format!("local path not found: {} ({})", relative_path, e))
    })?;
    let plan_abs = plan_dir.canonicalize().map_err(|e| {
        WrightError::ValidationError(format!(
            "failed to resolve plan directory {}: {}",
            plan_dir.display(),
            e
        ))
    })?;
    if !resolved.starts_with(&plan_abs) {
        return Err(WrightError::ValidationError(format!(
            "local path escapes plan directory: {}",
            relative_path
        )));
    }
    Ok(resolved)
}

/// Recursively hard-link all files from src_dir to dest_dir, preserving directory
/// structure and symbolic links.  Falls back to copy when a hard-link fails
/// (e.g. across btrfs subvolumes — see [`link_or_copy`]).
///
/// # Important: does not follow symlinks
///
/// This function uses `symlink_metadata` rather than `metadata` so that
/// symbolic links are reproduced in the output instead of being followed.
/// Following symlinks would incorrectly traverse into host directories
/// (e.g. `var/run -> /run`) and attempt to hard-link runtime sockets,
/// which fails with `ENXIO`.
async fn hard_link_all(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    let mut dirs_to_visit = vec![src_dir.to_path_buf()];
    while let Some(dir) = dirs_to_visit.pop() {
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();

                let file_type = match tokio::fs::symlink_metadata(&path).await {
                    Ok(m) => Some(m.file_type()),
                    Err(_) => None,
                };

                let Ok(rel_path) = path.strip_prefix(src_dir) else {
                    continue;
                };
                let dest_path = dest_dir.join(rel_path);
                if let Some(parent) = dest_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }

                match file_type {
                    Some(ft) if ft.is_symlink() => {
                        let target = tokio::fs::read_link(&path).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to read symlink {}: {}",
                                path.display(),
                                e
                            ))
                        })?;
                        tokio::fs::symlink(&target, &dest_path).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to create symlink {} -> {}: {}",
                                dest_path.display(),
                                target.display(),
                                e
                            ))
                        })?;
                    }
                    Some(ft) if ft.is_dir() => {
                        dirs_to_visit.push(path);
                    }
                    _ => {
                        if let Err(e) = link_or_copy(&path, &dest_path).await {
                            return Err(WrightError::ForgeError(format!(
                                "failed to link {} to {}: {}",
                                path.display(),
                                dest_path.display(),
                                e
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Hard-link `src` to `dest`, falling back to copy on EXDEV.
///
/// ## btrfs subvolume pitfall
///
/// Hard links fail with `EXDEV` across btrfs subvolume boundaries even when
/// both paths appear to be on the same filesystem (`st_dev` may differ even
/// within a single `btrfs` mount tree).  This is commonly triggered when:
///
/// 1. The host uses separate btrfs subvolumes for `/` and `/var`.
/// 2. The build staging directory sits under `/var/tmp/wright/...`.
/// 3. Strict-isolation overlayfs creates its upper layer on `/` (root subvolume).
/// 4. `make install` inside the isolation writes a directory through the
///    overlayfs merged view rather than through the `/output` bind-mount,
///    placing that directory's inode on the root subvolume while the staging
///    directory itself is on the `/var` subvolume.
/// 5. Output slicing attempts to hard-link files from staging into outputs/ —
///    the per-file inodes live on different subvolumes → `EXDEV`.
///
/// Falling back to `copy` is safe and functionally equivalent; it only costs
/// extra disk space for affected files (typically a small fraction of the
/// total build output).
async fn link_or_copy(src: &Path, dest: &Path) -> std::io::Result<()> {
    match tokio::fs::hard_link(src, dest).await {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            tokio::fs::copy(src, dest).await.map(|_| ())
        }
        Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
            tokio::fs::copy(src, dest).await.map(|_| ())
        }
        Err(e) => Err(e),
    }
}

impl Forger {
    pub fn new(config: GlobalConfig) -> Self {
        let mut executors = executor::ExecutorRegistry::new();
        if let Err(e) = executors.load_from_dir(&config.general.executors_dir) {
            tracing::warn!(
                "Failed to load executors from {}: {}",
                config.general.executors_dir.display(),
                e
            );
        }
        let network_pool = Arc::new(Semaphore::new(
            config.network.max_concurrent_downloads.max(1),
        ));
        Self {
            config,
            executors,
            network_pool,
        }
    }

    pub fn compute_build_key(&self, manifest: &PlanManifest) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(manifest.metadata.name.as_bytes());
        hasher.update(
            manifest
                .metadata
                .version
                .as_deref()
                .unwrap_or("")
                .as_bytes(),
        );
        hasher.update(manifest.metadata.release.to_string().as_bytes());
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
                        hasher.update(self.process_uri(r#ref, manifest).as_bytes());
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
        let mut stage_names: Vec<_> = manifest.pipeline.keys().collect();
        stage_names.sort();
        for name in stage_names {
            if let Some(stage) = manifest.pipeline.get(name) {
                hasher.update(name.as_bytes());
                hasher.update(stage.script.as_bytes());
                hasher.update(stage.executor.as_bytes());
            }
        }
        Ok(format!("{:x}", hasher.finalize()))
    }

    fn process_uri(&self, uri: &str, manifest: &PlanManifest) -> String {
        let mut vars = std::collections::HashMap::new();
        variables::insert_metadata_variables(
            &mut vars,
            &manifest.metadata.name,
            manifest.metadata.version.as_deref().unwrap_or(""),
            manifest.metadata.release,
            &manifest.metadata.arch,
        );
        variables::substitute(uri, &vars)
    }

    pub fn build_root(&self, manifest: &PlanManifest) -> Result<PathBuf> {
        let forge_dir = if self.config.build.forge_dir.is_absolute() {
            self.config.build.forge_dir.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| WrightError::ForgeError(format!("failed to get cwd: {}", e)))?
                .join(&self.config.build.forge_dir)
        };
        let ver = manifest.metadata.version.as_deref().unwrap_or("");
        if ver.is_empty() {
            Ok(forge_dir.join(format!("{}-noversion", manifest.metadata.name)))
        } else {
            Ok(forge_dir.join(format!("{}-{}", manifest.metadata.name, ver)))
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        base_root: &Path,
        stages: &[String],
        force_stage: &[String],
        until_stage: Option<&str>,
        fetch_only: bool,
        skip_check: bool,
        force: bool,
        clean: bool,
        extra_env: &std::collections::HashMap<String, String>,
        verbose: bool,
        nproc_per_isolation: Option<u32>,
        configure_lock: Option<Arc<Semaphore>>,
        compile_lock: Option<Arc<Semaphore>>,
    ) -> Result<ForgeResult> {
        if clean {
            self.clean(manifest).await?;
        }

        let build_root = self.build_root(manifest)?;
        let staging_dir = build_root.join("staging");
        let outputs_dir = build_root.join("outputs");
        let _default_output_dir = outputs_dir.join("default");
        let logs_dir = build_root.join("logs");
        let output_dir = staging_dir.clone();
        let partial = !stages.is_empty() || fetch_only;
        let build_key = self.compute_build_key(manifest)?;
        let build_phase = extra_env.get("WRIGHT_BUILD_PHASE").map(|s| s.as_str());

        if !stages.is_empty() && until_stage.is_some() {
            return Err(WrightError::ForgeError(
                "cannot combine --stage with --until-stage".to_string(),
            ));
        }

        if let Some(stage_name) = until_stage {
            let pipeline = pipeline::stage_order_for_manifest(manifest, build_phase);
            if !pipeline.iter().any(|stage| stage == stage_name) {
                return Err(WrightError::ForgeError(format!(
                    "stage '{}' not found in pipeline",
                    stage_name
                )));
            }
        }

        let key_file = build_root.join(".build_key");
        let extracted_marker = build_root.join(".extracted");
        let layers_dir = build_root.join("layers");

        if !stages.is_empty() {
            if !layers_dir.exists() {
                return Err(WrightError::ForgeError(
                    "cannot use --stage: no previous forge found (layers/ does not exist). Run a full forge first.".to_string()
                ));
            }
            ensure_clean_dir(&staging_dir).await?;
            ensure_clean_dir(&logs_dir).await?;
        } else {
            let work_reusable = layers_dir.exists()
                && extracted_marker.exists()
                && key_file.exists()
                && tokio::fs::read_to_string(&key_file)
                    .await
                    .map(|stored| stored.trim() == build_key)
                    .unwrap_or(false);

            if work_reusable {
                debug!("Source tree unchanged (forge key match) — reusing layers/");
                ensure_clean_dir(&staging_dir).await?;
                ensure_clean_dir(&logs_dir).await?;
            } else {
                ensure_clean_dir(&build_root).await?;
                ensure_clean_dir(&staging_dir).await?;
                ensure_clean_dir(&logs_dir).await?;
            }
        }

        if stages.is_empty() {
            if tokio::fs::metadata(&extracted_marker).await.is_err() {
                {
                    let _s = crate::cli_span!("Fetching", "{}", manifest.metadata.name);
                    self.fetch(manifest, plan_dir).await?;
                }
                if until_stage == Some("fetch") {
                    return Ok(ForgeResult {
                        output_dir: staging_dir.clone(),
                        work_dir: build_root.clone(),
                        logs_dir,
                        split_part_dirs: std::collections::HashMap::new(),
                    });
                }
                {
                    let _s = crate::cli_span!("Verifying", "{}", manifest.metadata.name);
                    self.verify(manifest).await?;
                }
                if until_stage == Some("verify") {
                    return Ok(ForgeResult {
                        output_dir: staging_dir.clone(),
                        work_dir: build_root.clone(),
                        logs_dir,
                        split_part_dirs: std::collections::HashMap::new(),
                    });
                }

                // Hardlink sources from global cache into the fetch layer.
                let lm = layers::LayerManager::new(&build_root)?;
                let fetch_layer = lm.ensure_fetch_layer()?;
                self.hardlink_sources_to_fetch_layer(manifest, &fetch_layer)
                    .await?;

                let extract_layer = lm.ensure_extract_layer()?;
                {
                    let _s = crate::cli_span!("Extracting", "{}", manifest.metadata.name);
                    self.extract(manifest, &extract_layer).await?;
                }
                tokio::fs::write(&extracted_marker, "").await.map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to write extraction marker {}: {}",
                        extracted_marker.display(),
                        e
                    ))
                })?;
                if let Err(e) = tokio::fs::write(&key_file, &build_key).await {
                    // Cache-marker write failure — purely an optimization
                    // miss for the next run; not user-actionable.
                    debug!("failed to write forge cache marker: {}", e);
                }
                if until_stage == Some("extract") {
                    return Ok(ForgeResult {
                        output_dir: staging_dir.clone(),
                        work_dir: build_root.clone(),
                        logs_dir,
                        split_part_dirs: std::collections::HashMap::new(),
                    });
                }
            } else {
                debug!("Sources already extracted — skipping fetch/verify/extract");
                self.ensure_source_layers(manifest, &build_root).await?;
                if matches!(until_stage, Some("fetch" | "verify" | "extract")) {
                    return Ok(ForgeResult {
                        output_dir: staging_dir.clone(),
                        work_dir: build_root.clone(),
                        logs_dir,
                        split_part_dirs: std::collections::HashMap::new(),
                    });
                }
            }
        }

        if fetch_only {
            return Ok(ForgeResult {
                output_dir: staging_dir.clone(),
                work_dir: build_root.clone(),
                logs_dir,
                split_part_dirs: std::collections::HashMap::new(),
            });
        }

        let rlimits = ResourceLimits {
            memory_mb: manifest
                .options
                .memory_limit
                .or(self.config.build.memory_limit),
            cpu_time_secs: manifest
                .options
                .cpu_time_limit
                .or(self.config.build.cpu_time_limit),
            timeout_secs: manifest.options.timeout.or(self.config.build.timeout),
        };

        let available = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        let total_cpus = if let Some(cap) = self.config.build.max_cpus {
            available.min(cap.max(1) as u32)
        } else {
            available
        };
        let cpu_count = nproc_per_isolation
            .or(self.config.build.nproc_per_isolation)
            .unwrap_or(total_cpus);

        let target_dir = build_root.join("target");

        let mut vars = variables::standard_variables(variables::VariableContext {
            name: &manifest.metadata.name,
            version: manifest.metadata.version.as_deref().unwrap_or(""),
            release: manifest.metadata.release,
            arch: &manifest.metadata.arch,
            workdir: &target_dir.to_string_lossy(),
            part_dir: &staging_dir.to_string_lossy(),
            main_part_name: &manifest.metadata.name,
            main_part_dir: &staging_dir.to_string_lossy(),
        });

        for (k, v) in &manifest.options.env {
            vars.insert(k.clone(), v.clone());
        }
        vars.extend(extra_env.iter().map(|(k, v)| (k.clone(), v.clone())));

        let mut pipeline = pipeline::Pipeline::new(pipeline::PipelineContext {
            manifest,
            vars: vars.clone(),
            working_dir: &build_root,
            logs_dir: &logs_dir,
            base_root: base_root.to_path_buf(),
            work_dir: build_root.clone(),
            output_dir: output_dir.clone(),
            stages: stages.to_vec(),
            force_stage: force_stage.to_vec(),
            stop_after_stage: until_stage.map(str::to_string),
            skip_check,
            force,
            executors: &self.executors,
            rlimits: rlimits.clone(),
            verbose,
            cpu_count: Some(cpu_count),
            configure_lock,
            compile_cpu_count: Some(total_cpus),
            compile_lock,
            build_key: build_key.clone(),
        })?;

        let plan_name = &manifest.metadata.name;
        let forge_t0 = std::time::Instant::now();
        info!(
            verb = "Forging",
            event = "forge.started",
            plan_name = %plan_name,
            "{}",
            plan_name,
        );
        if let Err(e) = pipeline.run().await {
            info!(
                event = "forge.failed",
                plan_name = %plan_name,
                error = %e,
                "Forge pipeline failed"
            );
            return Err(e);
        }
        let forge_elapsed = forge_t0.elapsed().as_secs_f64();
        // Per Rule B (implicit success): no per-package completion line.
        // The overall "Finished" line is emitted by the top-level install
        // operation once the whole workflow ends.
        info!(
            event = "forge.completed",
            plan_name = %plan_name,
            elapsed_secs = forge_elapsed,
            "forge completed"
        );

        if !partial && let Err(e) = tokio::fs::write(&key_file, &build_key).await {
            // Cache-marker write failure — internal optimization only.
            debug!(event = "forge.key_write_failed", plan_name = %plan_name, path = %key_file.display(), error = %e, "failed to write forge cache marker");
        }

        Ok(ForgeResult {
            output_dir: staging_dir,
            work_dir: build_root,
            logs_dir,
            split_part_dirs: std::collections::HashMap::new(),
        })
    }

    /// Ensure the built-in source layers exist for pipeline overlays.
    ///
    /// Older builds extracted directly into `build_root/source`.  Forced
    /// rebuilds can also clear `layers/` while leaving `.extracted` behind.
    /// In both cases, rebuild the fetch/extract layers from the global cache so
    /// later pipeline stages can see sources through the stage overlay.
    async fn ensure_source_layers(&self, manifest: &PlanManifest, build_root: &Path) -> Result<()> {
        let lm = layers::LayerManager::new(build_root)?;
        let fetch_layer = lm.layer_dir("fetch");
        let extract_layer = lm.layer_dir("extract");

        if fetch_layer.exists() && extract_layer.exists() {
            return Ok(());
        }

        info!(
            "[{}] repairing missing source layers for pipeline overlay",
            manifest.metadata.name
        );
        lm.clear_layer("fetch");
        lm.clear_layer("extract");

        let fetch_layer = lm.ensure_fetch_layer()?;
        self.hardlink_sources_to_fetch_layer(manifest, &fetch_layer)
            .await?;

        let extract_layer = lm.ensure_extract_layer()?;
        self.extract(manifest, &extract_layer).await?;
        Ok(())
    }

    /// Hard-link downloaded sources from the global cache into the fetch layer
    /// (`layers/01-fetch/`).  Because these are hard-links, deleting the sandbox
    /// removes only the link — the global cache file is never touched.
    async fn hardlink_sources_to_fetch_layer(
        &self,
        manifest: &PlanManifest,
        fetch_layer: &Path,
    ) -> Result<()> {
        let cache_dir = &self.config.general.source_dir;
        for source in &manifest.sources.entries {
            let (cache_path, _filename) = match source {
                Source::Http(http) => {
                    let processed_url = self.process_uri(&http.url, manifest);
                    let filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_url)
                    });
                    (cache_dir.join(&filename), filename)
                }
                Source::Local(local) => {
                    let processed_path = self.process_uri(&local.path, manifest);
                    let filename = source_cache_filename(&manifest.metadata.name, &processed_path);
                    (cache_dir.join(&filename), filename)
                }
                Source::Git(_) => {
                    // Git repos are cloned directly, not hardlinked.
                    continue;
                }
            };

            if tokio::fs::metadata(&cache_path).await.is_ok() {
                let dest = fetch_layer.join(
                    cache_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "source".to_string()),
                );
                debug!(
                    "Hard-linking {} -> {}",
                    cache_path.display(),
                    dest.display()
                );
                link_or_copy(&cache_path, &dest).await.map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to hard-link source to fetch layer {} -> {}: {}",
                        cache_path.display(),
                        dest.display(),
                        e
                    ))
                })?;
            } else {
                debug!(
                    "Source not in cache, skipping hardlink: {}",
                    cache_path.display()
                );
            }
        }
        Ok(())
    }

    /// Re-slice the staging directory into output directories based on the
    /// plan's `[[output]]` configuration.  This is the standalone version of
    /// the slicing logic previously inlined in `build`; it can be invoked by
    /// `wright package` to regenerate `outputs/` from an existing `staging/`.
    pub async fn slice_outputs(
        &self,
        manifest: &PlanManifest,
        build_root: &Path,
    ) -> Result<ForgeResult> {
        let staging_dir = build_root.join("staging");
        let outputs_dir = build_root.join("outputs");
        let default_output_dir = outputs_dir.join("default");
        let work_dir = build_root.to_path_buf();
        let logs_dir = build_root.join("logs");

        if !staging_dir.exists() {
            return Err(WrightError::ForgeError(format!(
                "staging directory does not exist: {}. Run `wright build {}` first.",
                staging_dir.display(),
                manifest.metadata.name
            )));
        }

        // Wipe and recreate outputs so that removed/changed patterns in the
        // plan do not leave stale files behind.
        ensure_clean_dir(&outputs_dir).await?;
        tokio::fs::create_dir_all(&default_output_dir)
            .await
            .map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to create default output directory {}: {}",
                    default_output_dir.display(),
                    e
                ))
            })?;

        let mut split_part_dirs = std::collections::HashMap::new();

        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            let has_catchall = parts.iter().any(|(_, sub_part)| sub_part.include.is_none());
            let mut sub_rules: Vec<(
                &str,
                PathBuf,
                Vec<globset::GlobMatcher>,
                Vec<globset::GlobMatcher>,
            )> = Vec::new();
            for (sub_name, sub_part) in parts {
                let incs = match &sub_part.include {
                    Some(v) => v,
                    None => continue, // catch-all: handled below
                };
                let sub_output_dir = outputs_dir.join(sub_name);
                tokio::fs::create_dir_all(&sub_output_dir)
                    .await
                    .map_err(|e| {
                        WrightError::ForgeError(format!(
                            "failed to create output directory {}: {}",
                            sub_output_dir.display(),
                            e
                        ))
                    })?;
                let includes = incs
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid include glob '{}' for {}: {}",
                                    pat, sub_name, e
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                let excludes = sub_part
                    .exclude
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid exclude glob '{}' for {}: {}",
                                    pat, sub_name, e
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                split_part_dirs.insert(sub_name.clone(), sub_output_dir.clone());
                sub_rules.push((sub_name.as_str(), sub_output_dir, includes, excludes));
            }

            let mut discard_rules: Vec<(
                &str,
                Vec<globset::GlobMatcher>,
                Vec<globset::GlobMatcher>,
            )> = Vec::new();
            for discard in &manifest.discard {
                let includes = discard
                    .include
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid discard include glob '{}': {}",
                                    pat, e
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                let excludes = discard
                    .exclude
                    .iter()
                    .map(|pat| {
                        globset::Glob::new(pat)
                            .map_err(|e| {
                                WrightError::ForgeError(format!(
                                    "invalid discard exclude glob '{}': {}",
                                    pat, e
                                ))
                            })
                            .map(|g| g.compile_matcher())
                    })
                    .collect::<Result<Vec<_>>>()?;
                discard_rules.push((discard.reason.as_str(), includes, excludes));
            }

            debug!(
                "Splitting staging dir into {} outputs and {} discard rules",
                sub_rules.len(),
                discard_rules.len()
            );
            let mut all_entries = Vec::new();
            let mut symlink_entries = Vec::new();
            let mut dirs_to_visit = vec![staging_dir.clone()];
            while let Some(dir) = dirs_to_visit.pop() {
                if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        let file_type = match tokio::fs::symlink_metadata(&path).await {
                            Ok(m) => m.file_type(),
                            Err(_) => continue,
                        };
                        if file_type.is_symlink() {
                            symlink_entries.push(path);
                        } else if file_type.is_dir() {
                            dirs_to_visit.push(path);
                        } else {
                            all_entries.push(path);
                        }
                    }
                }
            }
            all_entries.sort();
            symlink_entries.sort();

            let mut link_actions = Vec::new();
            let mut symlink_actions = Vec::new();
            let mut unmatched = Vec::new();

            // Helper: evaluate which outputs (if any) claim a given path.
            // Returns a vec of (output_name, output_dir) for every output whose
            // include patterns match and whose exclude patterns do NOT match.
            let find_matches = |rel_str: &str| -> Vec<(&str, &PathBuf)> {
                let mut matches = Vec::new();
                for (sub_name, sub_dir, includes, excludes) in &sub_rules {
                    let mut matched = includes.iter().any(|m| m.is_match(rel_str));
                    if matched
                        && !excludes.is_empty()
                        && excludes.iter().any(|m| m.is_match(rel_str))
                    {
                        matched = false;
                    }
                    if matched {
                        matches.push((*sub_name, sub_dir));
                    }
                }
                matches
            };

            for file_path in &all_entries {
                if let Ok(rel_path) = file_path.strip_prefix(&staging_dir) {
                    let rel_str = format!("/{}", rel_path.display());
                    let mut dest_path = None;
                    let matches = find_matches(&rel_str);
                    match matches.len() {
                        0 => {
                            let discarded =
                                discard_rules.iter().any(|(_reason, includes, excludes)| {
                                    let matched = includes.iter().any(|m| m.is_match(&rel_str));
                                    matched
                                        && (excludes.is_empty()
                                            || !excludes.iter().any(|m| m.is_match(&rel_str)))
                                });
                            if !discarded {
                                if has_catchall {
                                    dest_path = Some(default_output_dir.join(rel_path));
                                } else {
                                    unmatched.push(rel_str);
                                }
                            }
                        }
                        1 => {
                            dest_path = Some(matches[0].1.join(rel_path));
                        }
                        _ => {
                            let names: Vec<_> = matches.iter().map(|(n, _)| *n).collect();
                            return Err(WrightError::ForgeError(format!(
                                "ambiguous: file '{}' is matched by multiple outputs: {}. \
                                 Adjust include/exclude patterns so that each file is claimed by at most one output.",
                                rel_str,
                                names.join(", ")
                            )));
                        }
                    }

                    if let Some(dest_path) = dest_path {
                        link_actions.push((file_path.clone(), dest_path));
                    }
                }
            }

            for symlink_path in &symlink_entries {
                if let Ok(rel_path) = symlink_path.strip_prefix(&staging_dir) {
                    let rel_str = format!("/{}", rel_path.display());
                    let mut dest_path = None;
                    let matches = find_matches(&rel_str);
                    match matches.len() {
                        0 => {
                            let discarded =
                                discard_rules.iter().any(|(_reason, includes, excludes)| {
                                    let matched = includes.iter().any(|m| m.is_match(&rel_str));
                                    matched
                                        && (excludes.is_empty()
                                            || !excludes.iter().any(|m| m.is_match(&rel_str)))
                                });
                            if !discarded {
                                if has_catchall {
                                    dest_path = Some(default_output_dir.join(rel_path));
                                } else {
                                    unmatched.push(rel_str);
                                }
                            }
                        }
                        1 => {
                            dest_path = Some(matches[0].1.join(rel_path));
                        }
                        _ => {
                            let names: Vec<_> = matches.iter().map(|(n, _)| *n).collect();
                            return Err(WrightError::ForgeError(format!(
                                "ambiguous: file '{}' is matched by multiple outputs: {}. \
                                 Adjust include/exclude patterns so that each file is claimed by at most one output.",
                                rel_str,
                                names.join(", ")
                            )));
                        }
                    }

                    if let Some(dest_path) = dest_path {
                        let target = tokio::fs::read_link(symlink_path).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to read symlink {}: {}",
                                symlink_path.display(),
                                e
                            ))
                        })?;
                        symlink_actions.push((dest_path, target));
                    }
                }
            }

            if !unmatched.is_empty() {
                let shown = unmatched
                    .iter()
                    .take(50)
                    .map(|p| format!("  - {p}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let omitted = unmatched.len().saturating_sub(50);
                let suffix = if omitted > 0 {
                    format!("\n  ... and {omitted} more")
                } else {
                    String::new()
                };

                let _ = tokio::fs::create_dir_all(&logs_dir).await;
                let log_path = logs_dir.join("slice-errors.log");
                if let Ok(mut f) = std::fs::File::create(&log_path) {
                    use std::io::Write;
                    let _ = writeln!(f, "plan = {}", manifest.metadata.name);
                    let _ = writeln!(f, "staging_dir = {}", staging_dir.display());
                    let _ = writeln!(f, "unmatched_count = {}", unmatched.len());
                    let _ = writeln!(f);
                    for p in &unmatched {
                        let _ = writeln!(f, "{}", p);
                    }
                    tracing::info!("Full unmatched file list written to {}", log_path.display());
                }

                return Err(WrightError::ForgeError(format!(
                    "{} staging files are not claimed by any [[output]] or [[discard]] rule:\n{}{}\nAdd an [[output]] include pattern, add an explicit [[discard]] rule, or add a catch-all [[output]] with no include.\nFull list: {}",
                    unmatched.len(),
                    shown,
                    suffix,
                    log_path.display()
                )));
            }

            for (file_path, dest_path) in link_actions {
                if let Some(parent) = dest_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                if let Err(e) = link_or_copy(&file_path, &dest_path).await {
                    return Err(WrightError::ForgeError(format!(
                        "failed to link {} to {}: {}",
                        file_path.display(),
                        dest_path.display(),
                        e
                    )));
                }
            }

            for (dest_path, target) in symlink_actions {
                if let Some(parent) = dest_path.parent() {
                    let _ = tokio::fs::create_dir_all(parent).await;
                }
                tokio::fs::symlink(&target, &dest_path).await.map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to create symlink {} -> {}: {}",
                        dest_path.display(),
                        target.display(),
                        e
                    ))
                })?;
            }
        } else {
            hard_link_all(&staging_dir, &default_output_dir).await?;
        }

        Ok(ForgeResult {
            output_dir: default_output_dir,
            work_dir,
            logs_dir,
            split_part_dirs,
        })
    }

    pub async fn clean(&self, manifest: &PlanManifest) -> Result<()> {
        let build_root = self.build_root(manifest)?;
        if tokio::fs::metadata(&build_root).await.is_ok() {
            force_clean_dir(&build_root).await?;
            tracing::debug!("Removed forge directory: {}", build_root.display());
        }
        Ok(())
    }

    pub async fn verify(&self, manifest: &PlanManifest) -> Result<()> {
        let cache_dir = &self.config.general.source_dir;
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
            let processed_url = self.process_uri(&http.url, manifest);
            let filename = http
                .r#as
                .clone()
                .unwrap_or_else(|| source_cache_filename(&manifest.metadata.name, &processed_url));
            let path = cache_dir.join(&filename);
            if tokio::fs::metadata(&path).await.is_err() {
                return Err(WrightError::ValidationError(format!(
                    "source file missing: {}",
                    filename
                )));
            }
            let actual_hash = checksum::sha256_file(&path)?;
            if actual_hash != http.sha256 {
                return Err(WrightError::ValidationError(format!(
                    "SHA256 mismatch for {}:\n  expected: {}\n  actual:   {}",
                    filename, http.sha256, actual_hash
                )));
            }
            debug!("Verified source: {}", filename);
        }
        Ok(())
    }

    pub async fn extract(&self, manifest: &PlanManifest, dest_dir: &Path) -> Result<PathBuf> {
        let cache_dir = &self.config.general.source_dir;
        for source in &manifest.sources.entries {
            match source {
                Source::Git(git) => {
                    let processed_url = self.process_uri(&git.url, manifest);
                    let git_dir_name = git_cache_dir_name(&processed_url);
                    let cache_path = cache_dir.join("git").join(&git_dir_name);
                    let git_ref = git
                        .r#ref
                        .as_deref()
                        .map(|r| self.process_uri(r, manifest))
                        .unwrap_or_else(|| "HEAD".to_string());
                    let final_dest = if let Some(ref sub) = git.extract_to {
                        let p = dest_dir.join(sub);
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
                    let repo = git2::Repository::clone(cache_str, &final_dest).map_err(|e| {
                        WrightError::ForgeError(format!("local git clone failed: {}", e))
                    })?;
                    let (object, reference) = repo
                        .revparse_ext(&git_ref)
                        .or_else(|_| repo.revparse_ext(&format!("origin/{}", git_ref)))
                        .map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to resolve ref {}: {}",
                                git_ref, e
                            ))
                        })?;
                    repo.checkout_tree(&object, None).map_err(|e| {
                        WrightError::ForgeError(format!("git checkout failed: {}", e))
                    })?;
                    match reference {
                        Some(gref) => {
                            let ref_name = gref.name().ok_or_else(|| {
                                WrightError::ForgeError(
                                    "git reference name is non-UTF-8".to_string(),
                                )
                            })?;
                            repo.set_head(ref_name)
                        }
                        None => repo.set_head_detached(object.id()),
                    }
                    .map_err(|e| {
                        WrightError::ForgeError(format!("failed to update HEAD: {}", e))
                    })?;
                }
                Source::Http(http) => {
                    let processed_url = self.process_uri(&http.url, manifest);
                    let filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_url)
                    });
                    let cache_path = cache_dir.join(&filename);
                    let final_dest = if let Some(ref sub) = http.extract_to {
                        let p = dest_dir.join(sub);
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
                                "failed to extract source {}: {}",
                                filename, e
                            ))
                        })?;
                    } else {
                        let dest = final_dest.join(&filename);
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to copy non-archive source {} to work directory: {}",
                                filename, e
                            ))
                        })?;
                    }
                }
                Source::Local(local) => {
                    let processed_path = self.process_uri(&local.path, manifest);
                    let filename = source_cache_filename(&manifest.metadata.name, &processed_path);
                    let cache_path = cache_dir.join(&filename);
                    let final_dest = if let Some(ref sub) = local.extract_to {
                        let p = dest_dir.join(sub);
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
                                "failed to extract local source {}: {}",
                                filename, e
                            ))
                        })?;
                    } else {
                        let dest = final_dest.join(&filename);
                        tokio::fs::copy(&cache_path, &dest).await.map_err(|e| {
                            WrightError::ForgeError(format!(
                                "failed to copy local source {} to work directory: {}",
                                filename, e
                            ))
                        })?;
                    }
                }
            }
        }
        Ok(dest_dir.to_path_buf())
    }

    pub async fn update_hashes(&self, manifest: &PlanManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();
        let cache_dir = &self.config.general.source_dir;
        if tokio::fs::metadata(cache_dir).await.is_err() {
            tokio::fs::create_dir_all(cache_dir)
                .await
                .map_err(WrightError::IoError)?;
        }
        for source in manifest.sources.entries.iter() {
            match source {
                Source::Http(http) => {
                    let processed_url = self.process_uri(&http.url, manifest);
                    let cache_filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.metadata.name, &processed_url)
                    });
                    let cache_path = cache_dir.join(&cache_filename);
                    if tokio::fs::metadata(&cache_path).await.is_ok() {
                        debug!("Using cached source: {}", cache_filename);
                    } else {
                        info!("Downloading {}...", processed_url);
                        download::download_file(
                            &processed_url,
                            &cache_path,
                            self.config.network.download_timeout,
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
                    result = format!(
                        "{}{}{}",
                        &result[..m.start()],
                        replacement,
                        &result[m.end()..]
                    );
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
                .map(|h| format!("    \"{}\"", h))
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
                    c.insert_str(uris_match.end(), &format!("\n{}", replacement));
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

    async fn fetch_git_repo(
        &self,
        git_url: &str,
        git_ref: Option<&str>,
        depth: Option<u32>,
        dest: &Path,
        scope: &str,
    ) -> Result<String> {
        let actual_ref = git_ref.unwrap_or("HEAD");

        // Detect arbitrary commit hashes (40-char hex) and disable shallow clone
        // since --depth may not reach them.
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
                .map_err(|e| WrightError::ForgeError(format!("git init failed: {}", e)))?
        } else {
            git2::Repository::open_bare(dest)
                .map_err(|e| WrightError::ForgeError(format!("git open failed: {}", e)))?
        };

        // If the repository already exists and the ref resolves locally,
        // skip the network fetch entirely. This avoids redundant fetches
        // when the plan.toml has not changed.
        if !is_fresh_clone && let Ok(obj) = repo.revparse_single(actual_ref) {
            tracing::debug!(
                "[{}] git ref '{}' already available locally; skipping fetch",
                scope,
                actual_ref
            );
            return Ok(obj.id().to_string());
        }

        let mut remote = repo
            .remote_anonymous(git_url)
            .map_err(|e| WrightError::ForgeError(format!("git remote setup failed: {}", e)))?;
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

            // Calculate progress position/length across all phases.
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
        if let Some(d) = effective_depth
            && d > 0
        {
            fetch_opts.depth(d as i32);
        }
        let fetch_result = remote.fetch(
            &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
            Some(&mut fetch_opts),
            None,
        );
        if let Err(e) = fetch_result {
            return Err(WrightError::ForgeError(format!("git fetch failed: {e}")));
        }
        drop(git_span);
        let obj = repo.revparse_single(actual_ref).map_err(|e| {
            WrightError::ForgeError(format!("failed to resolve git ref '{}': {}", actual_ref, e))
        })?;
        Ok(obj.id().to_string())
    }

    pub async fn fetch(&self, manifest: &PlanManifest, plan_dir: &Path) -> Result<()> {
        let cache_dir = &self.config.general.source_dir;
        if tokio::fs::metadata(cache_dir).await.is_err() {
            tokio::fs::create_dir_all(cache_dir)
                .await
                .map_err(WrightError::IoError)?;
        }

        // Fan out: one future per source. Concurrency is bounded by the
        // network semaphore (config.network.max_concurrent_downloads).
        // For packages with one source the loop body just runs once;
        // for packages with many (kernel, gtk, etc.) the wall-clock
        // cost drops to roughly max(individual sources).
        let futs = manifest
            .sources
            .entries
            .iter()
            .map(|source| self.fetch_one_source(manifest, plan_dir, source));
        futures_util::future::try_join_all(futs).await?;
        Ok(())
    }

    async fn fetch_one_source(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        source: &Source,
    ) -> Result<()> {
        let cache_dir = &self.config.general.source_dir;
        match source {
            Source::Git(git) => {
                let _permit = self
                    .network_pool
                    .acquire()
                    .await
                    .expect("network semaphore closed");
                let processed_url = self.process_uri(&git.url, manifest);
                let git_dir_name = git_cache_dir_name(&processed_url);
                let git_cache_dir = cache_dir.join("git");
                if tokio::fs::metadata(&git_cache_dir).await.is_err() {
                    tokio::fs::create_dir_all(&git_cache_dir).await.ok();
                }
                let dest = git_cache_dir.join(&git_dir_name);
                let processed_ref = git.r#ref.as_deref().map(|r| self.process_uri(r, manifest));
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
                let processed_url = self.process_uri(&http.url, manifest);
                let filename = http.r#as.clone().unwrap_or_else(|| {
                    source_cache_filename(&manifest.metadata.name, &processed_url)
                });
                let dest = cache_dir.join(&filename);
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
                    // download::download_file is sync (reqwest::blocking),
                    // so push it to the blocking pool to keep the async
                    // runtime worker free.
                    let url = processed_url.clone();
                    let dest_owned = dest.clone();
                    let timeout = self.config.network.download_timeout;
                    let scope = manifest.metadata.name.clone();
                    tokio::task::spawn_blocking(move || {
                        download::download_file(&url, &dest_owned, timeout, &scope)
                    })
                    .await
                    .map_err(|e| WrightError::ForgeError(format!("download join: {}", e)))??;
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
                let processed_path = self.process_uri(&local.path, manifest);
                let local_path = validate_local_path(plan_dir, &processed_path)?;
                let filename = source_cache_filename(&manifest.metadata.name, &processed_path);
                let dest = cache_dir.join(&filename);
                let label = progress::source_label(&processed_path);
                let _span = crate::cli_span!(
                    "Fetching",
                    "{} ({})",
                    label,
                    manifest.metadata.name
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalConfig;
    use crate::plan::manifest::PlanManifest;
    use std::path::Path;

    #[tokio::test]
    async fn slice_outputs_rejects_ambiguous_overlap() {
        let manifest = PlanManifest::parse(
            r#"
name = "overlap"
version = "1.0.0"
release = 1
description = "test overlap detection"
license = "MIT"
arch = "x86_64"

[pipeline.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 /bin/sh ${STAGING_DIR}/usr/bin/overlap
"""

[[output]]
name = "bin"
description = "binaries"
include = ["/usr/bin/**"]

[[output]]
name = "all"
description = "everything"
include = ["/usr/**"]
"#,
        )
        .unwrap();

        let mut config = GlobalConfig::default();
        let build_tmp = tempfile::tempdir().unwrap();
        config.build.forge_dir = build_tmp.path().to_path_buf();

        let plan_dir = tempfile::tempdir().unwrap();
        let forger = Forger::new(config);
        forger
            .build(
                &manifest,
                plan_dir.path(),
                Path::new("/"),
                &[] as &[String],
                &[],
                None,
                false,
                false,
                false,
                false,
                &std::collections::HashMap::new(),
                false,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let build_root = forger.build_root(&manifest).unwrap();
        let result = forger.slice_outputs(&manifest, &build_root).await;

        let err = match result {
            Ok(_) => panic!("expected overlapping outputs to fail slicing"),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("ambiguous"),
            "error should mention ambiguity: {}",
            msg
        );
        assert!(
            msg.contains("/usr/bin/overlap"),
            "error should name the file: {}",
            msg
        );
        assert!(
            msg.contains("bin") && msg.contains("all"),
            "error should name both outputs: {}",
            msg
        );
    }
}
