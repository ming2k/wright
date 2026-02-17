pub mod lifecycle;
pub mod executor;
pub mod variables;

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::config::GlobalConfig;
use crate::error::{WrightError, Result};
use crate::package::manifest::PackageManifest;
use crate::repo::source::sanitize_cache_filename;
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
    uri.starts_with("http://") || uri.starts_with("https://")
}

/// Check whether a filename looks like a supported archive format.
fn is_archive(filename: &str) -> bool {
    filename.ends_with(".tar.gz")
        || filename.ends_with(".tgz")
        || filename.ends_with(".tar.xz")
        || filename.ends_with(".tar.bz2")
        || filename.ends_with(".tar.zst")
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
    pub fn build(
        &self,
        manifest: &PackageManifest,
        hold_dir: &Path,
        stop_after: Option<String>,
    ) -> Result<BuildResult> {
        let build_root = self.build_root(manifest)?;

        let src_dir = build_root.join("src");
        let pkg_dir = build_root.join("pkg");
        let log_dir = build_root.join("log");

        // Ensure clean build directories
        for dir in [&src_dir, &pkg_dir, &log_dir] {
            if dir.exists() {
                std::fs::remove_dir_all(dir).ok();
            }
            std::fs::create_dir_all(dir).map_err(|e| {
                WrightError::BuildError(format!(
                    "failed to create build directory {}: {}",
                    dir.display(),
                    e
                ))
            })?;
        }

        info!("Build directory: {}", build_root.display());

        // Fetch sources (remote downloads + local file copies to cache)
        self.fetch(manifest, hold_dir)?;

        // Verify sources
        self.verify(manifest)?;

        // Extract archives and copy non-archive files to files_dir
        let files_dir = build_root.join("files");
        let build_src_dir = self.extract(manifest, &src_dir, &files_dir)?;

        let files_dir_str = files_dir.to_string_lossy().to_string();

        let nproc = self.config.effective_jobs();

        let mut vars = variables::standard_variables(
            &manifest.plan.name,
            &manifest.plan.version,
            manifest.plan.release,
            &manifest.plan.arch,
            &src_dir.to_string_lossy(),
            &pkg_dir.to_string_lossy(),
            &files_dir_str,
            nproc,
            &self.config.build.cflags,
            &self.config.build.cxxflags,
        );
        vars.insert("BUILD_DIR".to_string(), build_src_dir.to_string_lossy().to_string());

        let vars_for_splits = vars.clone();

        let pipeline = lifecycle::LifecyclePipeline::new(
            manifest,
            vars,
            &src_dir,
            &log_dir,
            src_dir.clone(),
            pkg_dir.clone(),
            if files_dir.exists() { Some(files_dir.clone()) } else { None },
            stop_after,
            &self.executors,
        );

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

            info!("Running package stage for split: {}", split_name);

            let split_options = executor::ExecutorOptions {
                level: crate::sandbox::SandboxLevel::from_str(&package_stage.sandbox),
                src_dir: src_dir.clone(),
                pkg_dir: split_pkg_dir.clone(),
                files_dir: if files_dir.exists() { Some(files_dir.clone()) } else { None },
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
            let _ = std::fs::write(&log_path, &log_content);

            if result.exit_code != 0 {
                return Err(WrightError::BuildError(format!(
                    "split package '{}' packaging stage failed with exit code {}\nstderr: {}",
                    split_name, result.exit_code, result.stderr
                )));
            }

            split_pkg_dirs.insert(split_name.clone(), split_pkg_dir);
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
            let filename = sanitize_cache_filename(
                processed_uri.split('/').last().unwrap_or("source")
            );
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
            let filename = sanitize_cache_filename(
                processed_uri.split('/').last().unwrap_or("source")
            );
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

        // Detect the top-level source directory for BUILD_DIR.
        // If the archive extracted a single directory, point BUILD_DIR there.
        // Otherwise, BUILD_DIR is the extraction root itself.
        let entries: Vec<_> = std::fs::read_dir(dest_dir).map_err(|e| WrightError::IoError(e))?
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .collect();

        let build_dir = if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let dir = entries[0].path();
            info!("Source directory: {}", dir.display());
            dir
        } else {
            dest_dir.to_path_buf()
        };

        Ok(build_dir)
    }

    /// Update sha256 checksums in plan.toml.
    /// Only computes hashes for remote URIs; local paths get "SKIP".
    pub fn update_hashes(&self, manifest: &PackageManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();

        let cache_dir = self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir).map_err(|e| WrightError::IoError(e))?;
        }

        for uri in manifest.sources.uris.iter() {
            let processed_uri = self.process_uri(uri, manifest);

            if !is_remote_uri(&processed_uri) {
                // Local path — use SKIP
                new_hashes.push("SKIP".to_string());
                continue;
            }

            let cache_filename = sanitize_cache_filename(
                processed_uri.split('/').last().unwrap_or("source")
            );
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
        let content = std::fs::read_to_string(manifest_path).map_err(|e| WrightError::IoError(e))?;

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

        std::fs::write(manifest_path, new_content).map_err(|e| WrightError::IoError(e))?;

        Ok(())
    }

    /// Fetch sources for a package to the cache directory.
    /// Remote URIs are downloaded; local URIs are validated and copied to cache.
    pub fn fetch(&self, manifest: &PackageManifest, hold_dir: &Path) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(cache_dir).map_err(|e| WrightError::IoError(e))?;
        }

        for (i, uri) in manifest.sources.uris.iter().enumerate() {
            let processed_uri = self.process_uri(uri, manifest);

            if is_remote_uri(&processed_uri) {
                // Remote URI: download to cache
                let filename = sanitize_cache_filename(
                    processed_uri.split('/').last().unwrap_or("source")
                );
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
