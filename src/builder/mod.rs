pub mod executor;
pub mod lifecycle;
pub mod mvp;
pub mod orchestrator;
pub mod variables;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};

use crate::config::GlobalConfig;
use crate::dockyard::ResourceLimits;
use crate::error::{Result, WrightError};
use crate::plan::manifest::{FabricateConfig, PlanManifest};
use crate::repo::source::sanitize_cache_filename;
use crate::util::{checksum, compress, download, progress};

pub struct BuildResult {
    pub pkg_dir: PathBuf,
    pub src_dir: PathBuf,
    pub log_dir: PathBuf,
    pub build_dir: PathBuf,
    pub split_pkg_dirs: std::collections::HashMap<String, PathBuf>,
}

pub struct Builder {
    config: GlobalConfig,
    executors: executor::ExecutorRegistry,
}

/// Check whether a URI is remote (http/https).
fn is_remote_uri(uri: &str) -> bool {
    uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("git+")
}

/// Check whether a URI is a Git repository.
fn is_git_uri(uri: &str) -> bool {
    uri.starts_with("git+")
}

/// Compute the cache filename for a remote source URI.
/// Prefixes with the part name to prevent collisions between plans/parts
/// that use similarly-named upstream tarballs (e.g. GitHub archive v*.tar.gz).
fn source_cache_filename(pkg_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", pkg_name, basename))
}

/// Compute a stable, collision-free cache directory name for a git URI.
///
/// Uses `<stem>-<8-char hash>` where the hash is a short SHA256 of the
/// bare URL (prefix and ref fragment stripped). Two repos that share the
/// same last path segment but come from different remotes (e.g.
/// `org-a/mylib.git` vs `org-b/mylib.git`) will therefore never collide.
fn git_cache_dir_name(uri: &str) -> String {
    use sha2::{Digest, Sha256};
    let url = uri.strip_prefix("git+").unwrap_or(uri);
    let url = url.split('#').next().unwrap_or(url);
    let last_segment = url.split('/').next_back().unwrap_or("repo");
    let stem = sanitize_cache_filename(last_segment.strip_suffix(".git").unwrap_or(last_segment));
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let hash = format!("{:x}", h.finalize());
    format!("{}-{}", stem, &hash[..8])
}

/// Check whether a filename looks like a supported archive format.
fn is_archive(filename: &str) -> bool {
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

        // 2. Hash source URIs and their expected hashes
        for source in &manifest.sources.entries {
            hasher.update(source.uri.as_bytes());
            hasher.update(source.sha256.as_bytes());
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

        // 4. Hash global build flags (CFLAGS, etc.)
        hasher.update(self.config.build.cflags.as_bytes());
        hasher.update(self.config.build.cxxflags.as_bytes());

        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Process a URI by substituting variables like ${PART_VERSION}, ${PART_NAME}, etc.
    fn process_uri(&self, uri: &str, manifest: &PlanManifest) -> String {
        let mut vars = std::collections::HashMap::new();
        vars.insert("PART_NAME".to_string(), manifest.plan.name.clone());
        vars.insert("PART_VERSION".to_string(), manifest.plan.version.clone());
        vars.insert(
            "PART_RELEASE".to_string(),
            manifest.plan.release.to_string(),
        );
        vars.insert("PART_ARCH".to_string(), manifest.plan.arch.clone());
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
        force: bool,
        // Per-dockyard NPROC hint from the scheduler. Applied only when both the
        // plan and global config leave jobs at 0 (auto-detect), preventing
        // CPU oversubscription when multiple dockyards run simultaneously.
        nproc_per_dockyard: Option<u32>,
        // Compile-stage semaphore: when set, compile stages acquire this lock
        // so only one dockyard compiles at a time with full CPU access.
        compile_lock: Option<Arc<Mutex<()>>>,
        // Optional spinner for live stage progress (multi-dockyard builds).
        progress: Option<indicatif::ProgressBar>,
    ) -> Result<BuildResult> {
        let build_root = self.build_root(manifest)?;

        let src_dir = build_root.join("src");
        let pkg_dir = build_root.join("pkg");
        let log_dir = build_root.join("log");

        let partial = !stages.is_empty() || fetch_only;
        let is_bootstrap = extra_env
            .get("WRIGHT_BUILD_PHASE")
            .is_some_and(|phase| phase == "mvp");

        // --- Caching Logic (Step 1: Check) ---
        // Bootstrap builds are intentionally incomplete; never use or save cache.
        let build_key = self.compute_build_key(manifest)?;
        let cache_dir = self.config.general.cache_dir.join("builds");
        let cache_file = cache_dir.join(format!("{}-{}.tar.zst", manifest.plan.name, build_key));

        if !force && !is_bootstrap && !partial && cache_file.exists() {
            debug!(
                "Cache hit for {}: using pre-built artifacts",
                manifest.plan.name
            );

            // Recreate directories
            for dir in [&src_dir, &pkg_dir, &log_dir] {
                ensure_clean_dir(dir)?;
            }

            // Extract cache into build_root
            compress::extract_archive(&cache_file, &build_root)?;

            // Re-detect sub-part directories from the cached build_root
            let mut split_pkg_dirs = std::collections::HashMap::new();
            if let Some(FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
                for sub_name in pkgs.keys() {
                    if sub_name == &manifest.plan.name {
                        continue;
                    }
                    let sub_dir = build_root.join(format!("pkg-{}", sub_name));
                    if sub_dir.exists() {
                        split_pkg_dirs.insert(sub_name.clone(), sub_dir);
                    }
                }
            }

            return Ok(BuildResult {
                pkg_dir,
                src_dir,
                log_dir,
                build_dir: build_root,
                split_pkg_dirs,
            });
        }

        if !stages.is_empty() {
            // When running specific stages, validate that a previous build exists
            if !src_dir.exists() {
                return Err(WrightError::BuildError(
                    "cannot use --stage: no previous build found (src/ does not exist). Run a full build first.".to_string()
                ));
            }
            // Only recreate pkg_dir and log_dir for fresh output
            for dir in [&pkg_dir, &log_dir] {
                ensure_clean_dir(dir)?;
            }
        } else {
            // Check if src/ can be reused: if the build key matches the
            // previous build, skip re-extraction for an incremental build.
            let key_file = build_root.join(".build_key");
            let src_reusable = src_dir.exists()
                && key_file.exists()
                && std::fs::read_to_string(&key_file)
                    .map(|stored| stored.trim() == build_key)
                    .unwrap_or(false);

            if src_reusable {
                debug!("Source tree unchanged (build key match) — reusing src/");
                for dir in [&pkg_dir, &log_dir] {
                    ensure_clean_dir(dir)?;
                }
            } else {
                for dir in [&src_dir, &pkg_dir, &log_dir] {
                    ensure_clean_dir(dir)?;
                }
            }
        }

        debug!("Build directory: {}", build_root.display());

        let files_dir = build_root.join("files");

        if stages.is_empty() {
            if !src_dir.join(".extracted").exists() {
                // Fetch sources (remote downloads + local file copies to cache)
                self.fetch(manifest, plan_dir)?;

                // Verify sources
                self.verify(manifest)?;

                // Extract archives and copy non-archive files to files_dir
                self.extract(manifest, &src_dir, &files_dir)?;

                // Mark extraction complete so incremental builds can skip it
                let _ = std::fs::write(src_dir.join(".extracted"), "");
            } else {
                debug!("Sources already extracted — skipping fetch/verify/extract");
            }
        }

        if fetch_only {
            return Ok(BuildResult {
                pkg_dir,
                src_dir,
                log_dir,
                build_dir: build_root,
                split_pkg_dirs: std::collections::HashMap::new(),
            });
        }

        // Detect BUILD_DIR from extracted sources
        let build_src_dir = Self::detect_build_dir(&src_dir)?;

        let files_dir_str = files_dir.to_string_lossy().to_string();

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

        // Compute scheduler's CPU share for this dockyard and apply it as CPU
        // affinity on the dockyard process. Tools like `nproc` inside the
        // dockyard then return the correct count without any env var injection.
        let available = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        let total_cpus = if let Some(cap) = self.config.build.max_cpus {
            available.min(cap.max(1) as u32)
        } else {
            available
        };
        let cpu_count = nproc_per_dockyard
            .or(self.config.build.nproc_per_dockyard)
            .unwrap_or(total_cpus);

        let vars = variables::standard_variables(variables::VariableContext {
            part_name: &manifest.plan.name,
            part_version: &manifest.plan.version,
            part_release: manifest.plan.release,
            part_arch: &manifest.plan.arch,
            src_dir: &src_dir.to_string_lossy(),
            part_dir: &pkg_dir.to_string_lossy(),
            files_dir: &files_dir_str,
            cflags: &self.config.build.cflags,
            cxxflags: &self.config.build.cxxflags,
        });
        let mut vars = vars;
        vars.insert(
            "BUILD_DIR".to_string(),
            build_src_dir.to_string_lossy().to_string(),
        );

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
            working_dir: &src_dir,
            log_dir: &log_dir,
            base_root: base_root.to_path_buf(),
            src_dir: src_dir.clone(),
            part_dir: pkg_dir.clone(),
            files_dir: if files_dir.exists() {
                Some(files_dir.clone())
            } else {
                None
            },
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

                let sub_pkg_dir = build_root.join(format!("pkg-{}", sub_name));
                std::fs::create_dir_all(&sub_pkg_dir).map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to create sub-part directory {}: {}",
                        sub_pkg_dir.display(),
                        e
                    ))
                })?;

                let mut sub_vars = vars_for_splits.clone();
                sub_vars.insert(
                    "PART_DIR".to_string(),
                    sub_pkg_dir.to_string_lossy().to_string(),
                );
                sub_vars.insert("PART_NAME".to_string(), sub_name.clone());
                sub_vars.insert(
                    "MAIN_PART_DIR".to_string(),
                    pkg_dir.to_string_lossy().to_string(),
                );

                debug!("Running fabricate stage for sub-part: {}", sub_name);

                let sub_options = executor::ExecutorOptions {
                    level: sub_pkg.dockyard.parse().unwrap(),
                    base_root: base_root.to_path_buf(),
                    src_dir: src_dir.clone(),
                    part_dir: sub_pkg_dir.clone(),
                    files_dir: if files_dir.exists() {
                        Some(files_dir.clone())
                    } else {
                        None
                    },
                    rlimits: rlimits.clone(),
                    main_part_dir: Some(pkg_dir.clone()),
                    verbose,
                    cpu_count: Some(cpu_count),
                };

                let sub_executor = self.executors.get(&sub_pkg.executor).ok_or_else(|| {
                    WrightError::BuildError(format!("executor not found: {}", sub_pkg.executor))
                })?;

                let mut result = executor::execute_script(
                    sub_executor,
                    &sub_pkg.script,
                    &src_dir,
                    &sub_pkg.env,
                    &sub_vars,
                    &sub_options,
                )?;

                // Write log — stream from captured temp files
                let log_path = log_dir.join(format!("part-{}.log", sub_name));
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

                split_pkg_dirs.insert(sub_name.clone(), sub_pkg_dir);
            }
        }

        // --- Caching Logic (Step 2: Save) ---
        // Bootstrap builds are incomplete by design; skip saving to cache.
        if !is_bootstrap && !partial {
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                warn!(
                    "Failed to create build cache directory {}: {}",
                    cache_dir.display(),
                    e
                );
            }
            // For the cache, we only store pkg/, log/ and pkg-* directories.
            // We exclude src/ to keep the cache compact.
            // A dedicated "cache builder" temporary directory to collect these
            let tmp_cache_dir = tempfile::tempdir().map_err(WrightError::IoError)?;
            for entry in std::fs::read_dir(&build_root).map_err(WrightError::IoError)? {
                let entry = entry.map_err(WrightError::IoError)?;
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str == "pkg" || name_str == "log" || name_str.starts_with("pkg-") {
                    let dest = tmp_cache_dir.path().join(&name);
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        // Use shell CP for speed and robustness with symlinks
                        if let Err(e) = std::process::Command::new("cp")
                            .arg("-a")
                            .arg(entry.path())
                            .arg(&dest)
                            .status()
                        {
                            warn!(
                                "Failed to copy {} to build cache: {}",
                                entry.path().display(),
                                e
                            );
                        }
                    }
                }
            }

            if let Err(e) = compress::create_tar_zst(tmp_cache_dir.path(), &cache_file) {
                warn!(
                    "Failed to create build cache for {}: {}",
                    manifest.plan.name, e
                );
            } else {
                debug!(
                    "Saved build cache for {} at {}",
                    manifest.plan.name,
                    cache_file.display()
                );
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
            pkg_dir,
            src_dir,
            log_dir,
            build_dir: build_root,
            split_pkg_dirs,
        })
    }

    /// Clean the working directory and build cache entry for a part.
    ///
    /// Working directory (`build_dir/<name>-<version>/`) is removed so the
    /// next build re-extracts sources from scratch.
    ///
    /// Build cache entry (`cache_dir/builds/<name>-<key>.tar.zst`) is also
    /// removed so the next build cannot hit the cache and must compile fully.
    /// This is the primary reason to use `--clean`: the working directory is
    /// recreated at the start of every build anyway, but the build cache
    /// persists across runs and can only be invalidated by a key change or
    /// this explicit clean.
    pub fn clean(&self, manifest: &PlanManifest) -> Result<()> {
        // 1. Remove working directory.
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

        // 2. Remove build cache entry.
        let build_key = self.compute_build_key(manifest)?;
        let cache_file = self
            .config
            .general
            .cache_dir
            .join("builds")
            .join(format!("{}-{}.tar.zst", manifest.plan.name, build_key));
        if cache_file.exists() {
            std::fs::remove_file(&cache_file).map_err(|e| {
                WrightError::BuildError(format!(
                    "failed to remove build cache for {}: {}",
                    manifest.plan.name, e
                ))
            })?;
            tracing::info!("Cleared build cache for {}", manifest.plan.name);
        }

        Ok(())
    }

    /// Verify integrity of downloaded sources.
    /// Only verifies remote URIs (local paths use "SKIP").
    pub fn verify(&self, manifest: &PlanManifest) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");

        for (i, source) in manifest.sources.entries.iter().enumerate() {
            if source.sha256 == "SKIP" {
                debug!("Skipping verification for source {}", i);
                continue;
            }

            let processed_uri = self.process_uri(&source.uri, manifest);
            let filename = source_cache_filename(&manifest.plan.name, &processed_uri);
            let path = cache_dir.join(&filename);

            if !path.exists() {
                return Err(WrightError::ValidationError(format!(
                    "source file missing: {}",
                    filename
                )));
            }

            let actual_hash = checksum::sha256_file(&path)?;
            if actual_hash != source.sha256 {
                return Err(WrightError::ValidationError(format!(
                    "SHA256 mismatch for {}:\n  expected: {}\n  actual:   {}",
                    filename, source.sha256, actual_hash
                )));
            }
            debug!("Verified source: {}", filename);
        }

        Ok(())
    }

    /// Extract archives to the build directory and copy non-archive files to files_dir.
    /// Returns the path to the top-level source directory (for BUILD_DIR).
    pub fn extract(
        &self,
        manifest: &PlanManifest,
        dest_dir: &Path,
        files_dir: &Path,
    ) -> Result<PathBuf> {
        let cache_dir = &self.config.general.cache_dir.join("sources");

        for source in &manifest.sources.entries {
            let processed_uri = self.process_uri(&source.uri, manifest);

            if is_git_uri(&processed_uri) {
                let git_dir_name = git_cache_dir_name(&processed_uri);
                let cache_path = cache_dir.join("git").join(&git_dir_name);

                // Parse the ref
                let git_ref = if let Some(pos) = processed_uri.find('#') {
                    let r = processed_uri[pos + 1..].to_string();
                    let parts: Vec<&str> = r.split('=').collect();
                    if parts.len() == 2 {
                        parts[1].to_string()
                    } else {
                        r
                    }
                } else {
                    "HEAD".to_string()
                };

                let target_dir = dest_dir.join(&git_dir_name);
                debug!(
                    "Extracting Git repo to {} (ref: {})...",
                    target_dir.display(),
                    git_ref
                );

                // Open the cached bare repo and clone it locally to the target_dir
                let cache_str = cache_path.to_str().ok_or_else(|| {
                    WrightError::BuildError(format!(
                        "git cache path contains non-UTF-8 characters: {}",
                        cache_path.display()
                    ))
                })?;
                let repo = git2::Repository::clone(cache_str, &target_dir).map_err(|e| {
                    WrightError::BuildError(format!("local git clone failed: {}", e))
                })?;

                // Resolve and checkout the specific ref
                let (object, reference) = repo
                    .revparse_ext(&git_ref)
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

                continue;
            }

            let cache_filename = source_cache_filename(&manifest.plan.name, &processed_uri);
            let path = cache_dir.join(&cache_filename);

            if is_archive(&cache_filename) {
                debug!("Extracting {}...", cache_filename);
                compress::extract_archive(&path, dest_dir)?;
            } else {
                // Non-archive file: copy to files_dir using the original basename
                // so build scripts can reference it by its natural name (e.g. $FILES_DIR/config).
                std::fs::create_dir_all(files_dir).map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to create files directory {}: {}",
                        files_dir.display(),
                        e
                    ))
                })?;
                let dest_name = processed_uri
                    .split('/')
                    .next_back()
                    .unwrap_or(&processed_uri);
                let dest = files_dir.join(dest_name);
                std::fs::copy(&path, &dest).map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to copy {} to {}: {}",
                        path.display(),
                        dest.display(),
                        e
                    ))
                })?;
                debug!(
                    "Copied {} to files directory as {}",
                    cache_filename, dest_name
                );
            }
        }

        Self::detect_build_dir(dest_dir)
    }

    /// Detect the top-level source directory for BUILD_DIR.
    /// If the directory contains a single subdirectory, point BUILD_DIR there.
    /// Otherwise, BUILD_DIR is the directory itself.
    fn detect_build_dir(src_dir: &Path) -> Result<PathBuf> {
        let entries: Vec<_> = std::fs::read_dir(src_dir)
            .map_err(WrightError::IoError)?
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .collect();

        let build_dir =
            if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let dir = entries[0].path();
                debug!("Source directory: {}", dir.display());
                dir
            } else {
                src_dir.to_path_buf()
            };

        Ok(build_dir)
    }

    /// Update sha256 checksums in plan.toml.
    /// Only computes hashes for remote URIs; local paths get "SKIP".
    pub fn update_hashes(&self, manifest: &PlanManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();

        let cache_dir = self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir).map_err(WrightError::IoError)?;
        }

        for source in manifest.sources.entries.iter() {
            let processed_uri = self.process_uri(&source.uri, manifest);

            if !is_remote_uri(&processed_uri) {
                // Local path — use SKIP
                new_hashes.push("SKIP".to_string());
                continue;
            }

            if is_git_uri(&processed_uri) {
                // Git sources have no downloadable file to hash — use SKIP
                new_hashes.push("SKIP".to_string());
                continue;
            }

            let cache_filename = source_cache_filename(&manifest.plan.name, &processed_uri);
            let cache_path = cache_dir.join(&cache_filename);

            if cache_path.exists() {
                debug!("Using cached source: {}", cache_filename);
            } else {
                info!("Downloading {}...", processed_uri);
                download::download_file(
                    &processed_uri,
                    &cache_path,
                    self.config.network.download_timeout,
                )
                .map_err(|e| {
                    WrightError::BuildError(format!("Failed to download {}: {}", processed_uri, e))
                })?;
            }

            let hash = checksum::sha256_file(&cache_path)?;
            debug!("Computed hash: {}", hash);
            new_hashes.push(hash);
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
    fn fetch_git_repo(&self, uri: &str, dest: &Path) -> Result<String> {
        let uri_body = uri
            .strip_prefix("git+")
            .ok_or_else(|| WrightError::BuildError(format!("invalid git URI: {}", uri)))?;
        let (git_url, git_ref) = if let Some(pos) = uri_body.find('#') {
            (uri_body[..pos].to_string(), uri_body[pos + 1..].to_string())
        } else {
            (uri_body.to_string(), "HEAD".to_string())
        };

        let ref_parts: Vec<&str> = git_ref.split('=').collect();
        let actual_ref = if ref_parts.len() == 2 {
            ref_parts[1]
        } else {
            &git_ref
        };

        let label = progress::source_label(&git_url);

        let repo = if !dest.exists() {
            info!("Cloning Git repository (native): {}", git_url);
            git2::Repository::init_bare(dest)
                .map_err(|e| WrightError::BuildError(format!("git init failed: {}", e)))?
        } else {
            git2::Repository::open_bare(dest)
                .map_err(|e| WrightError::BuildError(format!("git open failed: {}", e)))?
        };

        let mut remote = repo
            .remote_anonymous(&git_url)
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
            )
            .map_err(|e| WrightError::BuildError(format!("git fetch failed: {}", e)));
        fetch_result?;
        progress::finish_source(&pb, &label, dest);

        // Resolve the ref to a commit
        let obj = repo.revparse_single(actual_ref).map_err(|e| {
            WrightError::BuildError(format!("failed to resolve git ref '{}': {}", actual_ref, e))
        })?;

        Ok(obj.id().to_string())
    }

    /// Fetch sources for a plan to the cache directory.
    /// Remote URIs are downloaded; local URIs are validated and copied to cache.
    pub fn fetch(&self, manifest: &PlanManifest, plan_dir: &Path) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(cache_dir).map_err(WrightError::IoError)?;
        }

        for source in &manifest.sources.entries {
            let processed_uri = self.process_uri(&source.uri, manifest);

            if is_git_uri(&processed_uri) {
                // Git repository handling
                let git_dir_name = git_cache_dir_name(&processed_uri);
                let git_cache_dir = cache_dir.join("git");
                if !git_cache_dir.exists() {
                    std::fs::create_dir_all(&git_cache_dir).ok();
                }
                let dest = git_cache_dir.join(&git_dir_name);

                let commit_id = self.fetch_git_repo(&processed_uri, &dest)?;
                debug!("Fetched Git commit: {} for {}", commit_id, git_dir_name);
                continue;
            }

            if is_remote_uri(&processed_uri) {
                // Remote URI: download to cache
                let filename = source_cache_filename(&manifest.plan.name, &processed_uri);
                let dest = cache_dir.join(&filename);

                let expected_hash = Some(source.sha256.as_str());
                let skip_verify = source.sha256 == "SKIP";

                let mut needs_download = true;

                if dest.exists() {
                    if skip_verify {
                        debug!("Source {} already cached (SKIP verification)", filename);
                        needs_download = false;
                    } else if let Some(hash) = expected_hash {
                        if let Ok(actual_hash) = checksum::sha256_file(&dest) {
                            if actual_hash == hash {
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
                    } else {
                        debug!("Source {} already cached (no hash to verify)", filename);
                        needs_download = false;
                    }
                }

                if needs_download {
                    download::download_file(
                        &processed_uri,
                        &dest,
                        self.config.network.download_timeout,
                    )?;

                    // Verify immediately after download
                    if !skip_verify {
                        if let Some(hash) = expected_hash {
                            let actual_hash = checksum::sha256_file(&dest)?;
                            if actual_hash != hash {
                                return Err(WrightError::ValidationError(format!(
                                    "Downloaded file {} failed verification!\n  Expected: {}\n  Actual:   {}",
                                    filename, hash, actual_hash
                                )));
                            }
                        }
                    }
                }
            } else {
                // Local URI: validate path is within plan dir and copy to cache
                let local_path = validate_local_path(plan_dir, &processed_uri)?;
                let filename = source_cache_filename(&manifest.plan.name, &processed_uri);
                let dest = cache_dir.join(&filename);

                if !dest.exists() {
                    let label = progress::source_label(&processed_uri);
                    let pb = progress::new_source_spinner(&label, "copying");
                    std::fs::copy(&local_path, &dest).map_err(|e| {
                        WrightError::BuildError(format!(
                            "failed to copy local file {} to cache: {}",
                            local_path.display(),
                            e
                        ))
                    })?;
                    progress::finish_source(&pb, &label, &dest);
                } else {
                    debug!("Local file {} already in cache", filename);
                }
            }
        }

        Ok(())
    }
}
