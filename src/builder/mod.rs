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

    /// Process a URL by substituting variables like ${version}, ${PKG_NAME}, etc.
    fn process_url(&self, url: &str, manifest: &PackageManifest) -> String {
        let mut vars = std::collections::HashMap::new();
        vars.insert("version".to_string(), manifest.package.version.clone());
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
        self.extract(manifest, &src_dir)?;

        // Determine patches directory (must be canonical absolute path for scripts running in src_dir)
        let hold_dir_abs = std::fs::canonicalize(hold_dir)
            .map_err(|e| WrightError::BuildError(format!("failed to resolve plan dir {}: {}", hold_dir.display(), e)))?;
        let patches_dir = hold_dir_abs.join("patches");
        let patches_dir_str = if patches_dir.exists() {
            patches_dir.to_string_lossy().to_string()
        } else {
            src_dir.join("patches").to_string_lossy().to_string()
        };

        let nproc = self.config.effective_jobs();

        let vars = variables::standard_variables(
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

        let pipeline = lifecycle::LifecyclePipeline::new(
            manifest,
            vars,
            &src_dir,
            &log_dir,
            src_dir.clone(),
            pkg_dir.clone(),
            if patches_dir.exists() { Some(patches_dir) } else { None },
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
            let processed_url = self.process_url(url, manifest);
            let filename = sanitize_cache_filename(
                processed_url.split('/').last().unwrap_or("source")
            );
            let path = cache_dir.join(&filename);

            if !path.exists() {
                return Err(WrightError::ValidationError(format!("source file missing: {}", filename)));
            }

            let actual_hash = checksum::sha256_file(&path)?;
            let expected_hash = manifest.sources.sha256.get(i)
                .ok_or_else(|| WrightError::ValidationError(format!("no sha256 hash provided for source {}", i)))?;

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

    /// Extract downloaded sources to the build directory
    pub fn extract(&self, manifest: &PackageManifest, dest_dir: &Path) -> Result<()> {
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

        // Post-extraction: Repeatedly flatten single-directory nesting.
        // Handles tarballs like runit that extract as admin/runit-2.1.2/src/...
        loop {
            let entries: Vec<_> = std::fs::read_dir(dest_dir).map_err(|e| WrightError::IoError(e))?
                .filter_map(|e| e.ok())
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .collect();

            if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let sub_dir = entries[0].path();
                info!("Flattening source directory: {}", sub_dir.display());

                for sub_entry in std::fs::read_dir(&sub_dir).map_err(|e| WrightError::IoError(e))? {
                    let sub_entry = sub_entry.map_err(|e| WrightError::IoError(e))?;
                    let target = dest_dir.join(sub_entry.file_name());
                    std::fs::rename(sub_entry.path(), target).map_err(|e| WrightError::IoError(e))?;
                }
                let _ = std::fs::remove_dir(sub_dir);
            } else {
                break;
            }
        }

        Ok(())
    }

    /// Update sha256 checksums in package.toml
    pub fn update_hashes(&self, manifest: &PackageManifest, manifest_path: &Path) -> Result<()> {
        let mut new_hashes = Vec::new();

        let temp_dir = tempfile::tempdir().map_err(|e| WrightError::IoError(e))?;

        for (i, url) in manifest.sources.urls.iter().enumerate() {
            let processed_url = self.process_url(url, manifest);
            info!("Downloading {}...", processed_url);

            let filename = processed_url.split('/').last().unwrap_or("source");
            let dest = temp_dir.path().join(format!("{}.{}", i, filename));

            download::download_file(&processed_url, &dest, self.config.network.download_timeout).map_err(|e| {
                WrightError::BuildError(format!("Failed to download {}: {}", processed_url, e))
            })?;
            let hash = checksum::sha256_file(&dest)?;
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

            let mut needs_download = true;

            if dest.exists() {
                // If we have a hash in the manifest, verify the cached file
                if let Some(expected_hash) = manifest.sources.sha256.get(i) {
                    if expected_hash != "TODO_UPDATE_HASH" {
                        if let Ok(actual_hash) = checksum::sha256_file(&dest) {
                            if &actual_hash == expected_hash {
                                info!("Source {} already cached and verified", filename);
                                needs_download = false;
                            } else {
                                warn!("Cached source {} is corrupted (hash mismatch), re-downloading...", filename);
                                let _ = std::fs::remove_file(&dest);
                            }
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
                
                // Verify immediately after download if hash is available
                if let Some(expected_hash) = manifest.sources.sha256.get(i) {
                    if expected_hash != "TODO_UPDATE_HASH" {
                        let actual_hash = checksum::sha256_file(&dest)?;
                        if &actual_hash != expected_hash {
                            return Err(WrightError::ValidationError(format!(
                                "Downloaded file {} failed verification!\n  Expected: {}\n  Actual:   {}",
                                filename, expected_hash, actual_hash
                            )));
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
