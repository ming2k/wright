pub mod executor;
pub mod lifecycle;
pub mod logging;
pub mod mvp;
pub mod orchestrator;
pub mod variables;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};

use crate::archive::resolver::sanitize_cache_filename;
use crate::config::GlobalConfig;
use crate::isolation::ResourceLimits;
use crate::error::{Result, WrightError};
use crate::plan::manifest::{FabricateConfig, PlanManifest, Source};
use crate::util::{checksum, compress, download, progress};

pub struct BuildResult {
    pub output_dir: PathBuf,
    pub work_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub split_pkg_dirs: std::collections::HashMap<String, PathBuf>,
}

pub struct Builder {
    config: GlobalConfig,
    executors: executor::ExecutorRegistry,
}

/// Compute the cache filename for a remote source URI.
/// Prefixes with the part name to prevent collisions between plans/parts
/// that use similarly-named upstream tarballs (e.g. GitHub archive v*.tar.gz).
fn source_cache_filename(pkg_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", pkg_name, basename))
}

/// Compute a stable, collision-free cache directory name for a git URL.
///
/// Uses `<stem>-<8-char hash>` where the hash is a short SHA256 of the
/// bare URL. Two repos that share the same last path segment but come
/// from different remotes (e.g. `org-a/mylib.git` vs `org-b/mylib.git`)
/// will therefore never collide.
fn git_cache_dir_name(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let last_segment = url.split('/').next_back().unwrap_or("repo");
    let stem = sanitize_cache_filename(last_segment.strip_suffix(".git").unwrap_or(last_segment));
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let hash = format!("{:x}", h.finalize());
    format!("{}-{}", stem, &hash[..8])
}

/// Check whether a filename looks like a supported archive format.
fn is_part_file(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".tgz")
        || filename.ends_with(".tar.xz")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.zst")
        || filename.ends_with(".tar.lz")
        || filename.ends_with(".zip")
}

/// Remove a directory if it exists (logs a warning on failure), then recreate it.
fn ensure_clean_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(dir) {
            warn!("Failed to clean directory {}: {}", dir.display(), e);
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| {
        WrightError::BuildError(format!(
            "failed to create build directory {}: {}",
            dir.display(),
            e
        ))
    })
}

/// Validate that a local URI resolves within the plan directory.
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

impl Builder {
    pub fn new(config: GlobalConfig) -> Self {
        let mut executors = executor::ExecutorRegistry::new();
        if let Err(e) = executors.load_from_dir(&config.general.executors_dir) {
            tracing::warn!(
                "Failed to load executors from {}: {}",
                config.general.executors_dir.display(),
                e
            );
        }
        Self { config, executors }
    }

    /// Compute a unique hash representing the entire build context.
    pub fn compute_build_key(&self, manifest: &PlanManifest) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();

        // 1. Hash the plan itself
        hasher.update(manifest.plan.name.as_bytes());
        hasher.update(manifest.plan.version.as_bytes());
        hasher.update(manifest.plan.release.to_string().as_bytes());

        // 2. Hash source configurations
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
                        hasher.update(&depth.to_le_bytes());
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

        // 3. Hash build instructions (lifecycle scripts)
        let mut stage_names: Vec<_> = manifest.lifecycle.keys().collect();
        stage_names.sort();
        for name in stage_names {
            if let Some(stage) = manifest.lifecycle.get(name) {
                hasher.update(name.as_bytes());
                hasher.update(stage.script.as_bytes());
                hasher.update(stage.executor.as_bytes());
            }
        }

        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Process a URI by substituting variables like ${VERSION}, ${NAME}, etc.
    fn process_uri(&self, uri: &str, manifest: &PlanManifest) -> String {
        let mut vars = std::collections::HashMap::new();
        variables::insert_metadata_variables(
            &mut vars,
            &manifest.plan.name,
            &manifest.plan.version,
            manifest.plan.release,
            &manifest.plan.arch,
        );
        variables::substitute(uri, &vars)
    }

    /// Get absolute build root for a part (tools like libtool require absolute paths).
    fn build_root(&self, manifest: &PlanManifest) -> Result<PathBuf> {
        let build_dir = if self.config.build.build_dir.is_absolute() {
            self.config.build.build_dir.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| WrightError::BuildError(format!("failed to get cwd: {}", e)))?
                .join(&self.config.build.build_dir)
        };
        Ok(build_dir.join(format!("{}-{}", manifest.plan.name, manifest.plan.version)))
    }

    /// Run the full build pipeline for a part manifest.
    /// Returns the BuildResult with paths to the build artifacts.
    ///
    /// `extra_env` is merged into every lifecycle stage's variable map.
    /// For MVP builds the orchestrator injects WRIGHT_BUILD_PHASE=mvp along
    /// with WRIGHT_BOOTSTRAP_WITHOUT_<DEP>=1.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &self,
        manifest: &PlanManifest,
        plan_dir: &Path,
        base_root: &Path,
        stages: &[String],
        fetch_only: bool,
        skip_check: bool,
        extra_env: &std::collections::HashMap<String, String>,
        verbose: bool,
        // Per-isolation NPROC hint from the scheduler. Applied only when both the
        // plan and global config leave jobs at 0 (auto-detect), preventing
        // CPU oversubscription when multiple isolations run simultaneously.
        nproc_per_isolation: Option<u32>,
        // Compile-stage semaphore: when set, compile stages acquire this lock
        // so only one isolation compiles at a time with full CPU access.
        compile_lock: Option<Arc<Mutex<()>>>,
        // Optional spinner for live stage progress (multi-isolation builds).
        progress: Option<indicatif::ProgressBar>,
    ) -> Result<BuildResult> {
        let build_root = self.build_root(manifest)?;

        let work_dir = build_root.join("work");
        let output_dir = build_root.join("output");
        let logs_dir = build_root.join("logs");

        let partial = !stages.is_empty() || fetch_only;
        let build_key = self.compute_build_key(manifest)?;

        if !stages.is_empty() {
            // When running specific stages, validate that a previous build exists
            if !work_dir.exists() {
                return Err(WrightError::BuildError(
                    "cannot use --stage: no previous build found (work/ does not exist). Run a full build first.".to_string()
                ));
            }
            // Only recreate output_dir and logs_dir for fresh output
            for dir in [&output_dir, &logs_dir] {
                ensure_clean_dir(dir)?;
            }
        } else {
            // Check if work/ can be reused: if the build key matches the
            // previous build, skip re-extraction for an incremental build.
            let key_file = build_root.join(".build_key");
            let work_reusable = work_dir.exists()
                && key_file.exists()
                && std::fs::read_to_string(&key_file)
                    .map(|stored| stored.trim() == build_key)
                    .unwrap_or(false);

            if work_reusable {
                debug!("Source tree unchanged (build key match) — reusing work/");
                for dir in [&output_dir, &logs_dir] {
                    ensure_clean_dir(dir)?;
                }
            } else {
                for dir in [&work_dir, &output_dir, &logs_dir] {
                    ensure_clean_dir(dir)?;
                }
            }
        }

        debug!("Build directory: {}", build_root.display());

        if stages.is_empty() {
            if !work_dir.join(".extracted").exists() {
                // Fetch sources (remote downloads + local file copies to cache)
                self.fetch(manifest, plan_dir)?;

                // Verify sources
                self.verify(manifest)?;

                // Extract parts and copy non-archive files to work_dir
                self.extract(manifest, &work_dir)?;

                // Mark extraction complete so incremental builds can skip it
                let _ = std::fs::write(work_dir.join(".extracted"), "");
            } else {
                debug!("Sources already extracted — skipping fetch/verify/extract");
            }
        }

        if fetch_only {
            return Ok(BuildResult {
                output_dir,
                work_dir,
                logs_dir,
                split_pkg_dirs: std::collections::HashMap::new(),
            });
        }

        // Resolve resource limits: per-plan overrides global config
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

        // Compute scheduler's CPU share for this isolation and apply it as CPU
        // affinity on the isolation process. Tools like `nproc` inside the
        // isolation then return the correct count without any env var injection.
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

        let mut vars = variables::standard_variables(variables::VariableContext {
            name: &manifest.plan.name,
            version: &manifest.plan.version,
            release: manifest.plan.release,
            arch: &manifest.plan.arch,
            workdir: &work_dir.to_string_lossy(),
            part_dir: &output_dir.to_string_lossy(),
            main_part_name: &manifest.plan.name,
            main_part_dir: &output_dir.to_string_lossy(),
        });

        // Package-level env from [options.env]: injected into all stages.
        // Per-stage env takes precedence (it goes in env_vars, which the
        // executor applies before vars).
        for (k, v) in &manifest.options.env {
            vars.insert(k.clone(), v.clone());
        }

        // Bootstrap / MVP env (highest priority among injected vars).
        vars.extend(extra_env.iter().map(|(k, v)| (k.clone(), v.clone())));

        let vars_for_splits = vars.clone();

        let pipeline = lifecycle::LifecyclePipeline::new(lifecycle::LifecycleContext {
            manifest,
            vars,
            working_dir: &work_dir,
            logs_dir: &logs_dir,
            base_root: base_root.to_path_buf(),
            work_dir: work_dir.clone(),
            output_dir: output_dir.clone(),
            stages: stages.to_vec(),
            skip_check,
            executors: &self.executors,
            rlimits: rlimits.clone(),
            verbose,
            cpu_count: Some(cpu_count),
            compile_cpu_count: Some(total_cpus),
            compile_lock,
            progress,
        });

        pipeline.run()?;

        // Run sub-part stages (multi-part mode)
        let mut split_pkg_dirs = std::collections::HashMap::new();
        if let Some(FabricateConfig::Multi(ref parts)) = manifest.fabricate {
            for (sub_name, sub_pkg) in parts {
                // Main part uses PART_DIR directly, skip
                if sub_name == &manifest.plan.name {
                    continue;
                }
                // Sub-parts with empty script use the main PART_DIR (no separate stage)
                if sub_pkg.script.is_empty() {
                    continue;
                }

                let sub_output_dir = build_root.join(format!("output-{}", sub_name));
                std::fs::create_dir_all(&sub_output_dir).map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to create sub-part directory {}: {}",
                        sub_output_dir.display(),
                        e
                    ))
                })?;

                let mut sub_vars = vars_for_splits.clone();
                sub_vars.insert(
                    "PART_DIR".to_string(),
                    sub_output_dir.to_string_lossy().to_string(),
                );
                sub_vars.insert("NAME".to_string(), sub_name.clone());
                sub_vars.insert(
                    "MAIN_PART_DIR".to_string(),
                    output_dir.to_string_lossy().to_string(),
                );

                debug!("Running fabricate stage for sub-part: {}", sub_name);

                let mut sub_options = executor::ExecutorOptions {
                    level: sub_pkg.isolation.parse()?,
                    base_root: base_root.to_path_buf(),
                    work_dir: work_dir.clone(),
                    output_dir: sub_output_dir.clone(),
                    rlimits: rlimits.clone(),
                    main_part_dir: Some(output_dir.clone()),
                    verbose,
                    cpu_count: Some(cpu_count),
                    log_stdout: None,
                };

                let sub_executor = self.executors.get(&sub_pkg.executor).ok_or_else(|| {
                    WrightError::BuildError(format!("executor not found: {}", sub_pkg.executor))
                })?;

                let mut result = executor::execute_script(
                    sub_executor,
                    &sub_pkg.script,
                    &work_dir,
                    &sub_pkg.env,
                    &sub_vars,
                    &mut sub_options,
                )?;

                // Write log — stream from captured temp files
                let log_path = logs_dir.join(format!("part-{}.log", sub_name));
                if let Ok(mut log_file) = std::fs::File::create(&log_path) {
                    use std::io::Write;
                    let _ = write!(
                        log_file,
                        "=== Sub-part: {} ===\n=== Exit code: {} ===\n\n",
                        sub_name, result.exit_code
                    );
                    let _ = log_file.write_all(b"--- stdout ---\n");
                    let _ = std::io::copy(&mut result.stdout.file, &mut log_file);
                    let _ = log_file.write_all(b"\n--- stderr ---\n");
                    let _ = std::io::copy(&mut result.stderr.file, &mut log_file);
                    let _ = log_file.write_all(b"\n");
                }

                if result.exit_code != 0 {
                    return Err(WrightError::BuildError(format!(
                        "sub-part '{}' packaging stage failed with exit code {}\nstderr: {}",
                        sub_name, result.exit_code, result.stderr.tail
                    )));
                }

                split_pkg_dirs.insert(sub_name.clone(), sub_output_dir);
            }
        }

        // Persist the build key so future runs can detect whether
        // the source tree is still valid for an incremental build.
        if !partial {
            let key_file = build_root.join(".build_key");
            if let Err(e) = std::fs::write(&key_file, &build_key) {
                warn!("Failed to write build key: {}", e);
            }
        }

        Ok(BuildResult {
            output_dir,
            work_dir,
            logs_dir,
            split_pkg_dirs,
        })
    }

    /// Clean the working directory for a part.
    ///
    /// Removes `build_dir/<name>-<version>/` so the next build re-extracts
    /// sources from scratch.
    pub fn clean(&self, manifest: &PlanManifest) -> Result<()> {
        let build_root = self.build_root(manifest)?;
        if build_root.exists() {
            std::fs::remove_dir_all(&build_root).map_err(|e| {
                WrightError::BuildError(format!(
                    "failed to clean build directory {}: {}",
                    build_root.display(),
                    e
                ))
            })?;
            tracing::debug!("Removed build directory: {}", build_root.display());
        }
        Ok(())
    }

    /// Verify integrity of downloaded sources.
    /// Only verifies `http` sources; `git` and `local` are skipped.
    pub fn verify(&self, manifest: &PlanManifest) -> Result<()> {
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
            let filename = http.r#as.clone().unwrap_or_else(|| {
                source_cache_filename(&manifest.plan.name, &processed_url)
            });
            let path = cache_dir.join(&filename);

            if !path.exists() {
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

    /// Extract parts to the work directory.
    /// Returns the path to the root work directory.
    pub fn extract(&self, manifest: &PlanManifest, dest_dir: &Path) -> Result<PathBuf> {
        let cache_dir = &self.config.general.source_dir;

        for source in &manifest.sources.entries {
            match source {
                Source::Git(git) => {
                    let processed_url = self.process_uri(&git.url, manifest);
                    let git_dir_name = git_cache_dir_name(&processed_url);
                    let cache_path = cache_dir.join("git").join(&git_dir_name);

                    let git_ref = git.r#ref.as_deref().unwrap_or("HEAD");

                    let final_dest = if let Some(ref sub) = git.extract_to {
                        let p = dest_dir.join(sub);
                        std::fs::create_dir_all(&p).map_err(WrightError::IoError)?;
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
                        WrightError::BuildError(format!(
                            "git cache path contains non-UTF-8 characters: {}",
                            cache_path.display()
                        ))
                    })?;

                    let repo = git2::Repository::clone(cache_str, &final_dest).map_err(|e| {
                        WrightError::BuildError(format!("local git clone failed: {}", e))
                    })?;

                    // Resolve and checkout the specific ref
                    let (object, reference) = repo
                        .revparse_ext(git_ref)
                        .or_else(|_| repo.revparse_ext(&format!("origin/{}", git_ref)))
                        .map_err(|e| {
                            WrightError::BuildError(format!("failed to resolve ref {}: {}", git_ref, e))
                        })?;

                    repo.checkout_tree(&object, None)
                        .map_err(|e| WrightError::BuildError(format!("git checkout failed: {}", e)))?;

                    match reference {
                        Some(gref) => {
                            let ref_name = gref.name().ok_or_else(|| {
                                WrightError::BuildError("git reference name is non-UTF-8".to_string())
                            })?;
                            repo.set_head(ref_name)
                        }
                        None => repo.set_head_detached(object.id()),
                    }
                    .map_err(|e| WrightError::BuildError(format!("failed to update HEAD: {}", e)))?;
                }
                Source::Http(http) => {
                    let processed_url = self.process_uri(&http.url, manifest);
                    let filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.plan.name, &processed_url)
                    });
                    let cache_path = cache_dir.join(&filename);

                    let final_dest = if let Some(ref sub) = http.extract_to {
                        let p = dest_dir.join(sub);
                        std::fs::create_dir_all(&p).map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.to_path_buf()
                    };

                    if is_part_file(&filename) {
                        let label = progress::source_label(&processed_url);
                        let pb = progress::new_source_spinner(&label, "extracting");
                        compress::extract_part(&cache_path, &final_dest).map_err(|e| {
                            WrightError::BuildError(format!(
                                "failed to extract source {}: {}",
                                filename, e
                            ))
                        })?;
                        progress::finish_source(&pb, &manifest.plan.name, &cache_path);
                    } else {
                        // Single file download: copy to final_dest
                        let dest = final_dest.join(&filename);
                        std::fs::copy(&cache_path, &dest).map_err(|e| {
                            WrightError::BuildError(format!(
                                "failed to copy non-archive source {} to work directory: {}",
                                filename, e
                            ))
                        })?;
                    }
                }
                Source::Local(local) => {
                    let processed_path = self.process_uri(&local.path, manifest);
                    let filename = source_cache_filename(&manifest.plan.name, &processed_path);
                    let cache_path = cache_dir.join(&filename);

                    let final_dest = if let Some(ref sub) = local.extract_to {
                        let p = dest_dir.join(sub);
                        std::fs::create_dir_all(&p).map_err(WrightError::IoError)?;
                        p
                    } else {
                        dest_dir.to_path_buf()
                    };

                    if is_part_file(&filename) {
                        let label = progress::source_label(&processed_path);
                        let pb = progress::new_source_spinner(&label, "extracting");
                        compress::extract_part(&cache_path, &final_dest).map_err(|e| {
                            WrightError::BuildError(format!(
                                "failed to extract local source {}: {}",
                                filename, e
                            ))
                        })?;
                        progress::finish_source(&pb, &manifest.plan.name, &cache_path);
                    } else {
                        // Single local file: copy to final_dest
                        let dest = final_dest.join(&filename);
                        std::fs::copy(&cache_path, &dest).map_err(|e| {
                            WrightError::BuildError(format!(
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

    /// Update sha256 checksums in plan.toml.
    /// Only computes hashes for HTTP sources; Git and local paths get "SKIP".
    pub fn update_hashes(&self, manifest: &PlanManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();

        let cache_dir = &self.config.general.source_dir;
        if !cache_dir.exists() {
            std::fs::create_dir_all(cache_dir).map_err(WrightError::IoError)?;
        }

        for source in manifest.sources.entries.iter() {
            match source {
                Source::Http(http) => {
                    let processed_url = self.process_uri(&http.url, manifest);
                    let cache_filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.plan.name, &processed_url)
                    });
                    let cache_path = cache_dir.join(&cache_filename);

                    if cache_path.exists() {
                        debug!("Using cached source: {}", cache_filename);
                    } else {
                        info!("Downloading {}...", processed_url);
                        download::download_file(
                            &processed_url,
                            &cache_path,
                            self.config.network.download_timeout,
                            &manifest.plan.name,
                        )
                        .map_err(|e| {
                            WrightError::BuildError(format!("Failed to download {}: {}", processed_url, e))
                        })?;
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

        // Surgical update of plan.toml
        let content = std::fs::read_to_string(manifest_path).map_err(WrightError::IoError)?;

        // Detect format: [[sources]] (array-of-tables) vs old [sources] with sha256 = [...]
        let has_array_of_tables = content.contains("[[sources]]");

        let new_content = if has_array_of_tables {
            // New format: update sha256 in each [[sources]] block
            let sha256_re = regex::Regex::new(r#"(?m)^(sha256\s*=\s*)"[^"]*""#).unwrap();
            let mut result = content.clone();
            let mut hash_idx = 0;
            // Replace each sha256 = "..." occurrence in order
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
            // Old format: update sha256 = [...] array
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
                    return Err(WrightError::BuildError(
                        "could not find sources or sha256 field in plan.toml".to_string(),
                    ));
                }
            }
        };

        std::fs::write(manifest_path, new_content).map_err(WrightError::IoError)?;

        Ok(())
    }

    /// Fetch a Git repository into the cache using native git2 library.
    fn fetch_git_repo(
        &self,
        git_url: &str,
        git_ref: Option<&str>,
        dest: &Path,
        scope: &str,
    ) -> Result<String> {
        let actual_ref = git_ref.unwrap_or("HEAD");

        let label = progress::source_label(git_url);

        let repo = if !dest.exists() {
            info!("[{}] Cloning Git repository: {}", scope, git_url);
            git2::Repository::init_bare(dest)
                .map_err(|e| WrightError::BuildError(format!("git init failed: {}", e)))?
        } else {
            git2::Repository::open_bare(dest)
                .map_err(|e| WrightError::BuildError(format!("git open failed: {}", e)))?
        };

        let mut remote = repo
            .remote_anonymous(git_url)
            .map_err(|e| WrightError::BuildError(format!("git remote setup failed: {}", e)))?;

        let pb = progress::new_source_transfer_bar(&label, 0);
        progress::set_source_git_objects(&pb, 0, 0, 0);

        let pb_clone = pb.clone();
        let mut callbacks = git2::RemoteCallbacks::new();
        callbacks.transfer_progress(move |stats| {
            let total = stats.total_objects() as u64;
            if total == 0 {
                return true;
            }
            progress::set_source_git_objects(
                &pb_clone,
                stats.received_objects() as u64,
                total,
                stats.received_bytes() as u64,
            );
            true
        });

        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.remote_callbacks(callbacks);
        fetch_opts.download_tags(git2::AutotagOption::All);

        let fetch_result = remote
            .fetch(
                &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
                Some(&mut fetch_opts),
                None,
            );

        if fetch_result.is_err() {
            return Err(WrightError::BuildError(format!(
                "git fetch failed: {}",
                fetch_result.unwrap_err()
            )));
        }

        // The bar reaches 100% after download, but indexing can take a long time for large repos.
        pb.set_message("indexing");
        
        progress::finish_source(&pb, scope, dest);

        // Resolve the ref to a commit
        let obj = repo.revparse_single(actual_ref).map_err(|e| {
            WrightError::BuildError(format!("failed to resolve git ref '{}': {}", actual_ref, e))
        })?;

        Ok(obj.id().to_string())
    }

    /// Fetch sources for a plan to the cache directory.
    pub fn fetch(&self, manifest: &PlanManifest, plan_dir: &Path) -> Result<()> {
        let cache_dir = &self.config.general.source_dir;
        if !cache_dir.exists() {
            std::fs::create_dir_all(cache_dir).map_err(WrightError::IoError)?;
        }

        for source in &manifest.sources.entries {
            match source {
                Source::Git(git) => {
                    let processed_url = self.process_uri(&git.url, manifest);
                    let git_dir_name = git_cache_dir_name(&processed_url);
                    let git_cache_dir = cache_dir.join("git");
                    if !git_cache_dir.exists() {
                        std::fs::create_dir_all(&git_cache_dir).ok();
                    }
                    let dest = git_cache_dir.join(&git_dir_name);

                    let commit_id =
                        self.fetch_git_repo(&processed_url, git.r#ref.as_deref(), &dest, &manifest.plan.name)?;
                    debug!("Fetched Git commit: {} for {}", commit_id, git_dir_name);
                }
                Source::Http(http) => {
                    let processed_url = self.process_uri(&http.url, manifest);
                    let filename = http.r#as.clone().unwrap_or_else(|| {
                        source_cache_filename(&manifest.plan.name, &processed_url)
                    });
                    let dest = cache_dir.join(&filename);

                    let skip_verify = http.sha256 == "SKIP";
                    let mut needs_download = true;

                    if dest.exists() {
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
                                let _ = std::fs::remove_file(&dest);
                            }
                        }
                    }

                    if needs_download {
                        download::download_file(
                            &processed_url,
                            &dest,
                            self.config.network.download_timeout,
                            &manifest.plan.name,
                        )?;

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
                    let filename = source_cache_filename(&manifest.plan.name, &processed_path);
                    let dest = cache_dir.join(&filename);

                    let label = progress::source_label(&processed_path);
                    let pb = progress::new_source_spinner(&label, "copying");
                    std::fs::copy(&local_path, &dest).map_err(|e| {
                        WrightError::BuildError(format!(
                            "failed to copy local file {} to cache: {}",
                            local_path.display(),
                            e
                        ))
                    })?;
                    progress::finish_source(&pb, &manifest.plan.name, &dest);
                }
            }
        }

        Ok(())
    }
}
