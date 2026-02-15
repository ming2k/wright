pub mod lifecycle;
pub mod executor;
pub mod variables;

use std::path::{Path, PathBuf};
use std::process::Command;

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
}

pub struct Builder {
    config: GlobalConfig,
    executors: executor::ExecutorRegistry,
}

impl Builder {
    pub fn new(config: GlobalConfig) -> Self {
        let mut executors = executor::ExecutorRegistry::new();
        if let Err(e) = executors.load_from_dir(&config.general.executors_dir) {
            tracing::warn!("Failed to load executors from {}: {}", config.general.executors_dir.display(), e);
        }
        Self { config, executors }
    }

    /// Process a URL by substituting variables like ${PKG_VERSION}, ${PKG_NAME}, etc.
    fn process_url(&self, url: &str, manifest: &PackageManifest) -> String {
        let mut vars = std::collections::HashMap::new();
        vars.insert("PKG_NAME".to_string(), manifest.package.name.clone());
        vars.insert("PKG_VERSION".to_string(), manifest.package.version.clone());
        vars.insert("PKG_RELEASE".to_string(), manifest.package.release.to_string());
        vars.insert("PKG_ARCH".to_string(), manifest.package.arch.clone());

        variables::substitute(url, &vars)
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
            manifest.package.name, manifest.package.version
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

        // Fetch sources
        self.fetch(manifest)?;

        // Verify sources
        self.verify(manifest)?;

        // Extract sources
        let build_src_dir = self.extract(manifest, &src_dir)?;

        // Fetch and apply patches
        let patches_dir = build_root.join("patches");
        self.fetch_patches(manifest, hold_dir, &patches_dir)?;
        self.apply_patches(&patches_dir, &build_src_dir)?;

        let patches_dir_str = patches_dir.to_string_lossy().to_string();

        let nproc = self.config.effective_jobs();

        let mut vars = variables::standard_variables(
            &manifest.package.name,
            &manifest.package.version,
            manifest.package.release,
            &manifest.package.arch,
            &src_dir.to_string_lossy(),
            &pkg_dir.to_string_lossy(),
            &patches_dir_str,
            nproc,
            &self.config.build.cflags,
            &self.config.build.cxxflags,
        );
        vars.insert("BUILD_DIR".to_string(), build_src_dir.to_string_lossy().to_string());

        let pipeline = lifecycle::LifecyclePipeline::new(
            manifest,
            vars,
            &src_dir,
            &log_dir,
            src_dir.clone(),
            pkg_dir.clone(),
            if patches_dir.exists() { Some(patches_dir.clone()) } else { None },
            stop_after,
            &self.executors,
        );

        pipeline.run()?;

        Ok(BuildResult {
            pkg_dir,
            src_dir,
            log_dir,
            build_dir: build_root,
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

    /// Verify integrity of downloaded sources
    pub fn verify(&self, manifest: &PackageManifest) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");

        for (i, url) in manifest.sources.urls.iter().enumerate() {
            let expected_hash = manifest.sources.sha256.get(i)
                .ok_or_else(|| WrightError::ValidationError(format!("no sha256 hash provided for source {}", i)))?;

            if expected_hash == "SKIP" {
                info!("Skipping verification for source {}", i);
                continue;
            }

            let processed_url = self.process_url(url, manifest);
            let filename = sanitize_cache_filename(
                processed_url.split('/').last().unwrap_or("source")
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

    /// Extract downloaded sources to the build directory.
    /// Returns the path to the top-level source directory (for BUILD_DIR).
    pub fn extract(&self, manifest: &PackageManifest, dest_dir: &Path) -> Result<PathBuf> {
        let cache_dir = &self.config.general.cache_dir.join("sources");

        for url in &manifest.sources.urls {
            let processed_url = self.process_url(url, manifest);
            let filename = sanitize_cache_filename(
                processed_url.split('/').last().unwrap_or("source")
            );
            let path = cache_dir.join(&filename);

            info!("Extracting {}...", filename);
            compress::extract_archive(&path, dest_dir)?;
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

    /// Update sha256 checksums in package.toml
    pub fn update_hashes(&self, manifest: &PackageManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();

        let cache_dir = self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(&cache_dir).map_err(|e| WrightError::IoError(e))?;
        }

        for url in manifest.sources.urls.iter() {
            let processed_url = self.process_url(url, manifest);
            let cache_filename = sanitize_cache_filename(
                processed_url.split('/').last().unwrap_or("source")
            );
            let cache_path = cache_dir.join(&cache_filename);

            if cache_path.exists() {
                info!("Using cached source: {}", cache_filename);
            } else {
                info!("Downloading {}...", processed_url);
                download::download_file(&processed_url, &cache_path, self.config.network.download_timeout).map_err(|e| {
                    WrightError::BuildError(format!("Failed to download {}: {}", processed_url, e))
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

        // Surgical update of package.toml using regex to preserve comments/formatting
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
            // If sha256 field is missing, try to insert it after urls
            let urls_re = regex::Regex::new(r"(?m)^urls\s*=\s*\[[\s\S]*?\]").unwrap();
            if urls_re.is_match(&content) {
                let urls_match = urls_re.find(&content).unwrap();
                let mut c = content.clone();
                c.insert_str(urls_match.end(), &format!("\n{}", replacement));
                c
            } else {
                return Err(WrightError::BuildError("could not find urls or sha256 field in package.toml".to_string()));
            }
        };

        std::fs::write(manifest_path, new_content).map_err(|e| WrightError::IoError(e))?;

        Ok(())
    }

    /// Fetch patches listed in the manifest into a consolidated patches directory.
    ///
    /// URL patches (http/https) are downloaded to the source cache and then copied.
    /// Local patches (relative paths) are resolved against `hold_dir`.
    /// All patches are placed in `patches_dir` with numeric prefixes to preserve
    /// manifest ordering.
    fn fetch_patches(
        &self,
        manifest: &PackageManifest,
        hold_dir: &Path,
        patches_dir: &Path,
    ) -> Result<()> {
        if manifest.sources.patches.is_empty() {
            return Ok(());
        }

        std::fs::create_dir_all(patches_dir).map_err(|e| {
            WrightError::BuildError(format!(
                "failed to create patches directory {}: {}",
                patches_dir.display(),
                e
            ))
        })?;

        let hold_dir_abs = std::fs::canonicalize(hold_dir).map_err(|e| {
            WrightError::BuildError(format!(
                "failed to resolve hold dir {}: {}",
                hold_dir.display(),
                e
            ))
        })?;

        let cache_dir = self.config.general.cache_dir.join("sources");
        std::fs::create_dir_all(&cache_dir).map_err(|e| WrightError::IoError(e))?;

        for (i, raw_patch) in manifest.sources.patches.iter().enumerate() {
            let patch_url = self.process_url(raw_patch, manifest);

            let source_path = if patch_url.starts_with("http://") || patch_url.starts_with("https://") {
                // Download URL patch to cache
                let filename = sanitize_cache_filename(
                    patch_url.split('/').last().unwrap_or("patch"),
                );
                let cached = cache_dir.join(&filename);

                if !cached.exists() {
                    info!("Fetching patch {}...", patch_url);
                    download::download_file(
                        &patch_url,
                        &cached,
                        self.config.network.download_timeout,
                    )?;
                } else {
                    info!("Patch {} already cached", filename);
                }
                cached
            } else {
                // Local patch – resolve relative to hold_dir
                let local = hold_dir_abs.join(&patch_url);
                if !local.exists() {
                    return Err(WrightError::BuildError(format!(
                        "local patch not found: {} (resolved to {})",
                        patch_url,
                        local.display()
                    )));
                }
                local
            };

            // Copy into consolidated patches dir with ordering prefix
            let original_name = source_path.file_name().unwrap_or_default().to_string_lossy().to_string();
            let dest_name = format!("{:04}-{}", i, original_name);
            let dest = patches_dir.join(&dest_name);
            std::fs::copy(&source_path, &dest).map_err(|e| {
                WrightError::BuildError(format!(
                    "failed to copy patch {} to {}: {}",
                    source_path.display(),
                    dest.display(),
                    e
                ))
            })?;

            // Create a symlink with the original filename so scripts can
            // reference patches by their unprefixed name via ${PATCHES_DIR}.
            let original_link = patches_dir.join(&original_name);
            if !original_link.exists() {
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(&dest_name, &original_link).map_err(|e| {
                        WrightError::BuildError(format!(
                            "failed to symlink {} -> {}: {}",
                            original_link.display(),
                            dest_name,
                            e
                        ))
                    })?;
                }
                #[cfg(not(unix))]
                {
                    std::fs::copy(&dest, &original_link).map_err(|e| {
                        WrightError::BuildError(format!(
                            "failed to copy patch {} to {}: {}",
                            dest.display(),
                            original_link.display(),
                            e
                        ))
                    })?;
                }
            }

            info!("Staged patch: {}", dest_name);
        }

        Ok(())
    }

    /// Apply all patches from `patches_dir` to `build_src_dir` using `patch -Np1`.
    ///
    /// Patches are applied in sorted filename order (the numeric prefix from
    /// `fetch_patches` ensures manifest ordering is respected).
    fn apply_patches(&self, patches_dir: &Path, build_src_dir: &Path) -> Result<()> {
        if !patches_dir.exists() {
            return Ok(());
        }

        let mut entries: Vec<_> = std::fs::read_dir(patches_dir)
            .map_err(|e| WrightError::IoError(e))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Only apply prefixed files (0000-*), skip original-name symlinks
                // to avoid double-application.
                (name.ends_with(".patch") || name.ends_with(".diff"))
                    && name.as_bytes().first().map_or(false, |b| b.is_ascii_digit())
            })
            .collect();

        if entries.is_empty() {
            return Ok(());
        }

        entries.sort_by_key(|e| e.file_name());

        for entry in &entries {
            let patch_path = entry.path();
            info!("Applying patch: {}...", patch_path.file_name().unwrap_or_default().to_string_lossy());

            let output = Command::new("patch")
                .args(["-Np1", "-i"])
                .arg(&patch_path)
                .current_dir(build_src_dir)
                .output()
                .map_err(|e| {
                    WrightError::BuildError(format!(
                        "failed to run patch command: {}",
                        e
                    ))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Err(WrightError::BuildError(format!(
                    "patch {} failed (exit {}):\n{}\n{}",
                    patch_path.display(),
                    output.status,
                    stdout,
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Fetch sources for a package to the cache directory
    pub fn fetch(&self, manifest: &PackageManifest) -> Result<()> {
        let cache_dir = &self.config.general.cache_dir.join("sources");
        if !cache_dir.exists() {
            std::fs::create_dir_all(cache_dir).map_err(|e| WrightError::IoError(e))?;
        }

        for (i, url) in manifest.sources.urls.iter().enumerate() {
            let processed_url = self.process_url(url, manifest);
            let filename = sanitize_cache_filename(
                processed_url.split('/').last().unwrap_or("source")
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
                info!("Fetching {} to {}", processed_url, dest.display());
                download::download_file(&processed_url, &dest, self.config.network.download_timeout)?;

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
        }

        Ok(())
    }
}
