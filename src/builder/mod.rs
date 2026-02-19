pub mod lifecycle;
pub mod executor;
pub mod variables;
pub mod orchestrator;

use std::path::{Path, PathBuf};

use tracing::{info, warn, debug};

use crate::config::GlobalConfig;
use crate::error::{WrightError, Result};
use crate::package::manifest::PackageManifest;
use crate::repo::source::sanitize_cache_filename;
use crate::sandbox::ResourceLimits;
use crate::util::{checksum, download, compress};

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
/// Prefixes with the package name to prevent collisions between packages
/// that use similarly-named upstream tarballs (e.g. GitHub archive v*.tar.gz).
fn source_cache_filename(pkg_name: &str, uri: &str) -> String {
    let basename = uri.split('/').next_back().unwrap_or("source");
    sanitize_cache_filename(&format!("{}-{}", pkg_name, basename))
}

/// Check whether a filename looks like a supported archive format.
fn is_archive(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".tgz")
        || filename.ends_with(".tar.xz")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.zst")
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
            dir.display(), e
        ))
    })
}

/// Validate that a local URI resolves within the plan directory.
fn validate_local_path(hold_dir: &Path, relative_path: &str) -> Result<PathBuf> {
    let resolved = hold_dir.join(relative_path).canonicalize().map_err(|e| {
        WrightError::ValidationError(format!(
            "local path not found: {} ({})",
            relative_path, e
        ))
    })?;
    let hold_abs = hold_dir.canonicalize().map_err(|e| {
        WrightError::ValidationError(format!(
            "failed to resolve plan directory {}: {}",
            hold_dir.display(), e
        ))
    })?;
    if !resolved.starts_with(&hold_abs) {
        return Err(WrightError::ValidationError(
            format!("local path escapes plan directory: {}", relative_path)
        ));
    }
    Ok(resolved)
}

impl Builder {
    pub fn new(config: GlobalConfig) -> Self {
        let mut executors = executor::ExecutorRegistry::new();
        if let Err(e) = executors.load_from_dir(&config.general.executors_dir) {
            tracing::warn!("Failed to load executors from {}: {}", config.general.executors_dir.display(), e);
        }
        Self { config, executors }
    }

    /// Compute a unique hash representing the entire build context.
    pub fn compute_build_key(&self, manifest: &PackageManifest) -> Result<String> {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();

        // 1. Hash the plan itself
        hasher.update(manifest.plan.name.as_bytes());
        hasher.update(manifest.plan.version.as_bytes());
        hasher.update(manifest.plan.release.to_string().as_bytes());

        // 2. Hash source URIs and their expected hashes
        for (i, uri) in manifest.sources.uris.iter().enumerate() {
            hasher.update(uri.as_bytes());
            if let Some(h) = manifest.sources.sha256.get(i) {
                hasher.update(h.as_bytes());
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

        // 4. Hash global build flags (CFLAGS, etc.)
        hasher.update(self.config.build.cflags.as_bytes());
        hasher.update(self.config.build.cxxflags.as_bytes());

        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Process a URI by substituting variables like ${PKG_VERSION}, ${PKG_NAME}, etc.
    fn process_uri(&self, uri: &str, manifest: &PackageManifest) -> String {
        let mut vars = std::collections::HashMap::new();
        vars.insert("PKG_NAME".to_string(), manifest.plan.name.clone());
        vars.insert("PKG_VERSION".to_string(), manifest.plan.version.clone());
        vars.insert("PKG_RELEASE".to_string(), manifest.plan.release.to_string());
        vars.insert("PKG_ARCH".to_string(), manifest.plan.arch.clone());

        variables::substitute(uri, &vars)
    }

    /// Get absolute build root for a package (tools like libtool require absolute paths).
    fn build_root(&self, manifest: &PackageManifest) -> Result<PathBuf> {
        let build_dir = if self.config.build.build_dir.is_absolute() {
            self.config.build.build_dir.clone()
        } else {
            std::env::current_dir()
                .map_err(|e| WrightError::BuildError(format!("failed to get cwd: {}", e)))?
                .join(&self.config.build.build_dir)
        };
        Ok(build_dir.join(format!(
            "{}-{}",
            manifest.plan.name, manifest.plan.version
        )))
    }

    /// Run the full build pipeline for a package manifest.
    /// Returns the BuildResult with paths to the build artifacts.
    ///
    /// `extra_env` is merged into every lifecycle stage's variable map.
    /// For MVP builds the orchestrator injects WRIGHT_BUILD_PHASE=mvp along
    /// with WRIGHT_BOOTSTRAP_BUILD=1 and WRIGHT_BOOTSTRAP_WITHOUT_<DEP>=1.
    pub fn build(
        &self,
        manifest: &PackageManifest,
        hold_dir: &Path,
        stop_after: Option<String>,
        only_stage: Option<String>,
        extra_env: &std::collections::HashMap<String, String>,
        verbose: bool,
    ) -> Result<BuildResult> {
        let build_root = self.build_root(manifest)?;

        let src_dir = build_root.join("src");
        let pkg_dir = build_root.join("pkg");
        let log_dir = build_root.join("log");

        let single_stage = only_stage.is_some();
        let is_bootstrap = extra_env.contains_key("WRIGHT_BOOTSTRAP_BUILD");

        // --- Caching Logic (Step 1: Check) ---
        // Bootstrap builds are intentionally incomplete; never use or save cache.
        let build_key = self.compute_build_key(manifest)?;
        let cache_dir = self.config.general.cache_dir.join("builds");
        let cache_file = cache_dir.join(format!("{}-{}.tar.zst", manifest.plan.name, build_key));

        if !is_bootstrap && !single_stage && stop_after.is_none() && cache_file.exists() {
            info!("Cache hit for {}: using pre-built artifacts", manifest.plan.name);
            
            // Recreate directories
            for dir in [&src_dir, &pkg_dir, &log_dir] {
                ensure_clean_dir(dir)?;
            }

            // Extract cache into build_root
            compress::extract_archive(&cache_file, &build_root)?;
            
            // Re-detect split package directories from the cached build_root
            let mut split_pkg_dirs = std::collections::HashMap::new();
            for split_name in manifest.split.keys() {
                let split_dir = build_root.join(format!("pkg-{}", split_name));
                if split_dir.exists() {
                    split_pkg_dirs.insert(split_name.clone(), split_dir);
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

        if single_stage {
            // When running a single stage, validate that a previous build exists
            if !src_dir.exists() {
                return Err(WrightError::BuildError(
                    "cannot use --only: no previous build found (src/ does not exist). Run a full build first.".to_string()
                ));
            }
            // Only recreate pkg_dir and log_dir for fresh output
            for dir in [&pkg_dir, &log_dir] {
                ensure_clean_dir(dir)?;
            }
            info!("Running only stage: {}", only_stage.as_deref().unwrap());
        } else {
            // Ensure clean build directories
            for dir in [&src_dir, &pkg_dir, &log_dir] {
                ensure_clean_dir(dir)?;
            }
        }

        info!("Build directory: {}", build_root.display());

        let files_dir = build_root.join("files");

        if !single_stage {
            // Fetch sources (remote downloads + local file copies to cache)
            self.fetch(manifest, hold_dir)?;

            // Verify sources
            self.verify(manifest)?;

            // Extract archives and copy non-archive files to files_dir
            self.extract(manifest, &src_dir, &files_dir)?;
        }

        // Detect BUILD_DIR from extracted sources
        let build_src_dir = Self::detect_build_dir(&src_dir)?;

        let files_dir_str = files_dir.to_string_lossy().to_string();

        // Resolve resource limits: per-plan overrides global config
        let rlimits = ResourceLimits {
            memory_mb: manifest.options.memory_limit.or(self.config.build.memory_limit),
            cpu_time_secs: manifest.options.cpu_time_limit.or(self.config.build.cpu_time_limit),
            timeout_secs: manifest.options.timeout.or(self.config.build.timeout),
        };

        // Per-plan jobs override global setting
        let nproc = if let Some(plan_jobs) = manifest.options.jobs {
            if plan_jobs == 0 {
                self.config.effective_jobs()
            } else {
                plan_jobs
            }
        } else {
            self.config.effective_jobs()
        };

        let vars = variables::standard_variables(variables::VariableContext {
            pkg_name: &manifest.plan.name,
            pkg_version: &manifest.plan.version,
            pkg_release: manifest.plan.release,
            pkg_arch: &manifest.plan.arch,
            src_dir: &src_dir.to_string_lossy(),
            pkg_dir: &pkg_dir.to_string_lossy(),
            files_dir: &files_dir_str,
            nproc,
            cflags: &self.config.build.cflags,
            cxxflags: &self.config.build.cxxflags,
        });
        let mut vars = vars;
        vars.insert("BUILD_DIR".to_string(), build_src_dir.to_string_lossy().to_string());
        // Inject bootstrap env vars (WRIGHT_BOOTSTRAP_BUILD, WRIGHT_BOOTSTRAP_WITHOUT_*)
        vars.extend(extra_env.iter().map(|(k, v)| (k.clone(), v.clone())));

        let vars_for_splits = vars.clone();

        let pipeline = lifecycle::LifecyclePipeline::new(lifecycle::LifecycleContext {
            manifest,
            vars,
            working_dir: &src_dir,
            log_dir: &log_dir,
            src_dir: src_dir.clone(),
            pkg_dir: pkg_dir.clone(),
            files_dir: if files_dir.exists() { Some(files_dir.clone()) } else { None },
            stop_after: stop_after.clone(),
            only_stage: only_stage.clone(),
            executors: &self.executors,
            rlimits: rlimits.clone(),
            verbose,
        });

        pipeline.run()?;

        // Run split package stages
        let mut split_pkg_dirs = std::collections::HashMap::new();
        for (split_name, split_pkg) in &manifest.split {
            let split_pkg_dir = build_root.join(format!("pkg-{}", split_name));
            std::fs::create_dir_all(&split_pkg_dir).map_err(|e| {
                WrightError::BuildError(format!(
                    "failed to create split package directory {}: {}",
                    split_pkg_dir.display(), e
                ))
            })?;

            let package_stage = split_pkg.lifecycle.get("package")
                .ok_or_else(|| WrightError::ValidationError(format!(
                    "split package '{}': lifecycle.package stage is required", split_name
                )))?;

            let mut split_vars = vars_for_splits.clone();
            split_vars.insert("PKG_DIR".to_string(), split_pkg_dir.to_string_lossy().to_string());
            split_vars.insert("PKG_NAME".to_string(), split_name.clone());
            split_vars.insert("MAIN_PKG_DIR".to_string(), pkg_dir.to_string_lossy().to_string());

            info!("Running package stage for split: {}", split_name);

            let split_options = executor::ExecutorOptions {
                level: package_stage.sandbox.parse().unwrap(),
                src_dir: src_dir.clone(),
                pkg_dir: split_pkg_dir.clone(),
                files_dir: if files_dir.exists() { Some(files_dir.clone()) } else { None },
                rlimits: rlimits.clone(),
                main_pkg_dir: Some(pkg_dir.clone()),
                verbose,
            };

            let split_executor = self.executors.get(&package_stage.executor)
                .ok_or_else(|| WrightError::BuildError(format!(
                    "executor not found: {}", package_stage.executor
                )))?;

            let result = executor::execute_script(
                split_executor,
                &package_stage.script,
                &src_dir,
                &package_stage.env,
                &split_vars,
                &split_options,
            )?;

            // Write log
            let log_path = log_dir.join(format!("package-{}.log", split_name));
            let log_content = format!(
                "=== Split package: {} ===\n=== Exit code: {} ===\n\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
                split_name, result.exit_code, result.stdout, result.stderr
            );
            if let Err(e) = std::fs::write(&log_path, &log_content) {
                warn!("Failed to write build log {}: {}", log_path.display(), e);
            }

            if result.exit_code != 0 {
                return Err(WrightError::BuildError(format!(
                    "split package '{}' packaging stage failed with exit code {}\nstderr: {}",
                    split_name, result.exit_code, result.stderr
                )));
            }

            split_pkg_dirs.insert(split_name.clone(), split_pkg_dir);
        }

        // --- Caching Logic (Step 2: Save) ---
        // Bootstrap builds are incomplete by design; skip saving to cache.
        if !is_bootstrap && !single_stage && stop_after.is_none() {
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                warn!("Failed to create build cache directory {}: {}", cache_dir.display(), e);
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
                            warn!("Failed to copy {} to build cache: {}", entry.path().display(), e);
                        }
                    }
                }
            }

            if let Err(e) = compress::create_tar_zst(tmp_cache_dir.path(), &cache_file) {
                warn!("Failed to create build cache for {}: {}", manifest.plan.name, e);
            } else {
                debug!("Saved build cache for {} at {}", manifest.plan.name, cache_file.display());
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

    /// Clean the build directory for a package.
    pub fn clean(&self, manifest: &PackageManifest) -> Result<()> {
        let build_root = self.build_root(manifest)?;
        if build_root.exists() {
            std::fs::remove_dir_all(&build_root).map_err(|e| {
                WrightError::BuildError(format!(
                    "failed to clean build directory {}: {}",
                    build_root.display(),
                    e
                ))
            })?;
        }
        Ok(())
    }

    /// Verify integrity of downloaded sources.
    /// Only verifies remote URIs (local paths use "SKIP").
    pub fn verify(&self, manifest: &PackageManifest) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");

        for (i, uri) in manifest.sources.uris.iter().enumerate() {
            let expected_hash = manifest.sources.sha256.get(i)
                .ok_or_else(|| WrightError::ValidationError(format!("no sha256 hash provided for source {}", i)))?;

            if expected_hash == "SKIP" {
                info!("Skipping verification for source {}", i);
                continue;
            }

            let processed_uri = self.process_uri(uri, manifest);
            let filename = source_cache_filename(&manifest.plan.name, &processed_uri);
            let path = cache_dir.join(&filename);

            if !path.exists() {
                return Err(WrightError::ValidationError(format!("source file missing: {}", filename)));
            }

            let actual_hash = checksum::sha256_file(&path)?;
            if &actual_hash != expected_hash {
                return Err(WrightError::ValidationError(format!(
                    "SHA256 mismatch for {}:\n  expected: {}\n  actual:   {}",
                    filename, expected_hash, actual_hash
                )));
            }
            info!("Verified source: {}", filename);
        }

        Ok(())
    }

    /// Extract archives to the build directory and copy non-archive files to files_dir.
    /// Returns the path to the top-level source directory (for BUILD_DIR).
    pub fn extract(&self, manifest: &PackageManifest, dest_dir: &Path, files_dir: &Path) -> Result<PathBuf> {
        let cache_dir = &self.config.general.cache_dir.join("sources");

        for uri in &manifest.sources.uris {
            let processed_uri = self.process_uri(uri, manifest);

            if is_git_uri(&processed_uri) {
                let base_uri = processed_uri.split('#').next().unwrap_or(&processed_uri);
                let last_segment = base_uri.split('/').next_back()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| WrightError::BuildError(
                        format!("cannot derive directory name from git URI: {}", processed_uri)
                    ))?;
                let git_dir_name = sanitize_cache_filename(
                    last_segment.strip_suffix(".git").unwrap_or(last_segment)
                );
                let cache_path = cache_dir.join("git").join(&git_dir_name);
                
                // Parse the ref
                let git_ref = if let Some(pos) = processed_uri.find('#') {
                    let r = processed_uri[pos+1..].to_string();
                    let parts: Vec<&str> = r.split('=').collect();
                    if parts.len() == 2 { parts[1].to_string() } else { r }
                } else {
                    "HEAD".to_string()
                };

                let target_dir = dest_dir.join(&git_dir_name);
                info!("Extracting Git repo to {} (ref: {})...", target_dir.display(), git_ref);

                // Open the cached bare repo and clone it locally to the target_dir
                let cache_str = cache_path.to_str()
                    .ok_or_else(|| WrightError::BuildError(
                        format!("git cache path contains non-UTF-8 characters: {}", cache_path.display())
                    ))?;
                let repo = git2::Repository::clone(cache_str, &target_dir)
                    .map_err(|e| WrightError::BuildError(format!("local git clone failed: {}", e)))?;

                // Resolve and checkout the specific ref
                let (object, reference) = repo.revparse_ext(&git_ref)
                    .or_else(|_| repo.revparse_ext(&format!("origin/{}", git_ref)))
                    .map_err(|e| WrightError::BuildError(format!("failed to resolve ref {}: {}", git_ref, e)))?;

                repo.checkout_tree(&object, None)
                    .map_err(|e| WrightError::BuildError(format!("git checkout failed: {}", e)))?;

                match reference {
                    Some(gref) => {
                        let ref_name = gref.name()
                            .ok_or_else(|| WrightError::BuildError(
                                "git reference name is non-UTF-8".to_string()
                            ))?;
                        repo.set_head(ref_name)
                    }
                    None => repo.set_head_detached(object.id()),
                }.map_err(|e| WrightError::BuildError(format!("failed to update HEAD: {}", e)))?;
                
                continue;
            }

            let filename = source_cache_filename(&manifest.plan.name, &processed_uri);
            let path = cache_dir.join(&filename);

            if is_archive(&filename) {
                info!("Extracting {}...", filename);
                compress::extract_archive(&path, dest_dir)?;
            } else {
                // Non-archive file: copy to files_dir
                std::fs::create_dir_all(files_dir).map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to create files directory {}: {}",
                        files_dir.display(), e
                    ))
                })?;
                let dest = files_dir.join(&filename);
                std::fs::copy(&path, &dest).map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to copy {} to {}: {}",
                        path.display(), dest.display(), e
                    ))
                })?;
                info!("Copied {} to files directory", filename);
            }
        }

        Self::detect_build_dir(dest_dir)
    }

    /// Detect the top-level source directory for BUILD_DIR.
    /// If the directory contains a single subdirectory, point BUILD_DIR there.
    /// Otherwise, BUILD_DIR is the directory itself.
    fn detect_build_dir(src_dir: &Path) -> Result<PathBuf> {
        let entries: Vec<_> = std::fs::read_dir(src_dir).map_err(WrightError::IoError)?
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .collect();

        let build_dir = if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let dir = entries[0].path();
            info!("Source directory: {}", dir.display());
            dir
        } else {
            src_dir.to_path_buf()
        };

        Ok(build_dir)
    }

    /// Update sha256 checksums in plan.toml.
    /// Only computes hashes for remote URIs; local paths get "SKIP".
    pub fn update_hashes(&self, manifest: &PackageManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();

        let cache_dir = self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir).map_err(WrightError::IoError)?;
        }

        for uri in manifest.sources.uris.iter() {
            let processed_uri = self.process_uri(uri, manifest);

            if !is_remote_uri(&processed_uri) {
                // Local path — use SKIP
                new_hashes.push("SKIP".to_string());
                continue;
            }

            let cache_filename = source_cache_filename(&manifest.plan.name, &processed_uri);
            let cache_path = cache_dir.join(&cache_filename);

            if cache_path.exists() {
                info!("Using cached source: {}", cache_filename);
            } else {
                info!("Downloading {}...", processed_uri);
                download::download_file(&processed_uri, &cache_path, self.config.network.download_timeout).map_err(|e| {
                    WrightError::BuildError(format!("Failed to download {}: {}", processed_uri, e))
                })?;
            }

            let hash = checksum::sha256_file(&cache_path)?;
            info!("Computed hash: {}", hash);
            new_hashes.push(hash);
        }

        if new_hashes.is_empty() {
            info!("No sources to update.");
            return Ok(());
        }

        // Surgical update of plan.toml using regex to preserve comments/formatting
        let content = std::fs::read_to_string(manifest_path).map_err(WrightError::IoError)?;

        let re = regex::Regex::new(r"(?m)^sha256\s*=\s*\[[\s\S]*?\]").unwrap();
        let hashes_str = new_hashes.iter()
            .map(|h| format!("    \"{}\"", h))
            .collect::<Vec<_>>()
            .join(",\n");
        let replacement = format!("sha256 = [\n{},\n]", hashes_str);

        let new_content = if re.is_match(&content) {
            re.replace(&content, &replacement).to_string()
        } else {
            // If sha256 field is missing, try to insert it after uris
            let uris_re = regex::Regex::new(r"(?m)^uris\s*=\s*\[[\s\S]*?\]").unwrap();
            if uris_re.is_match(&content) {
                let uris_match = uris_re.find(&content).unwrap();
                let mut c = content.clone();
                c.insert_str(uris_match.end(), &format!("\n{}", replacement));
                c
            } else {
                return Err(WrightError::BuildError("could not find uris or sha256 field in plan.toml".to_string()));
            }
        };

        std::fs::write(manifest_path, new_content).map_err(WrightError::IoError)?;

        Ok(())
    }

    /// Fetch a Git repository into the cache using native git2 library.
    fn fetch_git_repo(&self, uri: &str, dest: &Path) -> Result<String> {
        let uri_body = uri.strip_prefix("git+")
            .ok_or_else(|| WrightError::BuildError(format!("invalid git URI: {}", uri)))?;
        let (git_url, git_ref) = if let Some(pos) = uri_body.find('#') {
            (uri_body[..pos].to_string(), uri_body[pos+1..].to_string())
        } else {
            (uri_body.to_string(), "HEAD".to_string())
        };

        let ref_parts: Vec<&str> = git_ref.split('=').collect();
        let actual_ref = if ref_parts.len() == 2 { ref_parts[1] } else { &git_ref };

        let repo = if !dest.exists() {
            info!("Cloning Git repository (native): {}", git_url);
            git2::Repository::init_bare(dest)
                .map_err(|e| WrightError::BuildError(format!("git init failed: {}", e)))?
        } else {
            git2::Repository::open_bare(dest)
                .map_err(|e| WrightError::BuildError(format!("git open failed: {}", e)))?
        };

        let mut remote = repo.remote_anonymous(&git_url)
            .map_err(|e| WrightError::BuildError(format!("git remote setup failed: {}", e)))?;

        // Configure fetch options (e.g. proxy support can be added here)
        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.download_tags(git2::AutotagOption::All);

        info!("Fetching from remote: {}", git_url);
        remote.fetch(&["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"], Some(&mut fetch_opts), None)
            .map_err(|e| WrightError::BuildError(format!("git fetch failed: {}", e)))?;

        // Resolve the ref to a commit
        let obj = repo.revparse_single(actual_ref)
            .map_err(|e| WrightError::BuildError(format!("failed to resolve git ref '{}': {}", actual_ref, e)))?;
        
        Ok(obj.id().to_string())
    }

    /// Fetch sources for a package to the cache directory.
    /// Remote URIs are downloaded; local URIs are validated and copied to cache.
    pub fn fetch(&self, manifest: &PackageManifest, hold_dir: &Path) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(cache_dir).map_err(WrightError::IoError)?;
        }

        for (i, uri) in manifest.sources.uris.iter().enumerate() {
            let processed_uri = self.process_uri(uri, manifest);

            if is_git_uri(&processed_uri) {
                // Git repository handling
                let git_dir_name = sanitize_cache_filename(
                    processed_uri.split('#').next().unwrap()
                        .split('/').next_back().unwrap()
                        .strip_suffix(".git").unwrap_or(processed_uri.split('/').next_back().unwrap())
                );
                let git_cache_dir = cache_dir.join("git");
                if !git_cache_dir.exists() { std::fs::create_dir_all(&git_cache_dir).ok(); }
                let dest = git_cache_dir.join(&git_dir_name);

                let commit_id = self.fetch_git_repo(&processed_uri, &dest)?;
                info!("Fetched Git commit: {} for {}", commit_id, git_dir_name);
                continue;
            }

            if is_remote_uri(&processed_uri) {
                // Remote URI: download to cache
                let filename = source_cache_filename(&manifest.plan.name, &processed_uri);
                let dest = cache_dir.join(&filename);

                let expected_hash = manifest.sources.sha256.get(i).map(|s| s.as_str());
                let skip_verify = expected_hash == Some("SKIP");

                let mut needs_download = true;

                if dest.exists() {
                    if skip_verify {
                        info!("Source {} already cached (SKIP verification)", filename);
                        needs_download = false;
                    } else if let Some(hash) = expected_hash {
                        if let Ok(actual_hash) = checksum::sha256_file(&dest) {
                            if actual_hash == hash {
                                info!("Source {} already cached and verified", filename);
                                needs_download = false;
                            } else {
                                warn!("Cached source {} hash mismatch, re-downloading...", filename);
                                let _ = std::fs::remove_file(&dest);
                            }
                        }
                    } else {
                        info!("Source {} already cached (no hash to verify)", filename);
                        needs_download = false;
                    }
                }

                if needs_download {
                    info!("Fetching {} to {}", processed_uri, dest.display());
                    download::download_file(&processed_uri, &dest, self.config.network.download_timeout)?;

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
                let local_path = validate_local_path(hold_dir, &processed_uri)?;
                let filename = sanitize_cache_filename(
                    local_path.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("source")
                );
                let dest = cache_dir.join(&filename);

                if !dest.exists() {
                    std::fs::copy(&local_path, &dest).map_err(|e| {
                        WrightError::BuildError(format!(
                            "failed to copy local file {} to cache: {}",
                            local_path.display(), e
                        ))
                    })?;
                    info!("Copied local file {} to cache", processed_uri);
                } else {
                    info!("Local file {} already in cache", filename);
                }
            }
        }

        Ok(())
    }
}
