use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::error::{Result, WrightError};

/// Content-Addressed Storage for sealed `.part` archives.
///
/// After resolve identifies what needs building and forge+seal complete, the
/// fingerprint-based filename in a global read-only store.  On restart, the
/// resolver queries the store for pre-built parts — if they exist, forge and
/// seal are skipped entirely.
///
/// # Fingerprint composition
///
/// A part's CAS fingerprint captures the full transitive build closure:
///
///   sha256(
///     plan.build_key()           // sources + pipeline scripts
///     + dep1.fingerprint         // fingerprint of build dependency 1
///     + dep2.fingerprint         // fingerprint of build dependency 2
///     + ...
///   )
///
/// This ensures that a change anywhere in the dependency tree invalidates the
/// cached part for all dependents.
pub struct CasStore {
    store_dir: PathBuf,
}

impl CasStore {
    pub fn new(store_dir: PathBuf) -> Self {
        Self { store_dir }
    }

    /// Compute the closure fingerprint for a plan given its own build key and
    /// the fingerprints of its direct build dependencies.
    ///
    /// The dependencies map should contain `<dep_plan_name> -> <fingerprint>`.
    /// Dependencies are sorted for deterministic hashing order.
    pub fn compute_closure_fingerprint(
        build_key: &str,
        dep_fingerprints: &HashMap<String, String>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(build_key.as_bytes());

        let mut sorted_deps: Vec<_> = dep_fingerprints.iter().collect();
        sorted_deps.sort_by(|a, b| a.0.cmp(b.0));
        for (dep_name, dep_fp) in sorted_deps {
            hasher.update(b"\n");
            hasher.update(dep_name.as_bytes());
            hasher.update(b" ");
            hasher.update(dep_fp.as_bytes());
        }

        format!("{:x}", hasher.finalize())
    }

    /// Compute the store filename for a part.
    pub fn store_filename(name: &str, fingerprint: &str) -> String {
        format!("{}-{}.part", &fingerprint[..16], name)
    }

    /// Check whether a part with the given name and fingerprint exists in the
    /// CAS store.  Returns the path to the `.part` file if found.
    pub fn resolve(&self, name: &str, fingerprint: &str) -> Option<PathBuf> {
        let filename = Self::store_filename(name, fingerprint);
        let path = self.store_dir.join(&filename);
        if path.exists() {
            // Verify file integrity with a basic size check: a valid
            // .wright.tar.zst archive is always non-empty.
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > 0 => {
                    debug!("CAS hit: {} ({} bytes)", path.display(), meta.len());
                    Some(path)
                }
                Ok(_) => {
                    debug!("CAS miss: {} (empty file, discarding)", path.display());
                    None
                }
                Err(_) => None,
            }
        } else {
            debug!("CAS miss: {} (not in store)", filename);
            None
        }
    }

    /// Store a `.wright.tar.zst` part archive in the CAS store.
    ///
    /// The part is hard-linked if possible (same filesystem), falling back to
    /// copy, to avoid duplicating disk usage.
    pub fn store(&self, part_path: &Path, name: &str, fingerprint: &str) -> Result<PathBuf> {
        let filename = Self::store_filename(name, fingerprint);
        let dest = self.store_dir.join(&filename);

        // Ensure the store directory exists.
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to create CAS store directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        // If destination already exists with the same content, skip.
        if dest.exists() {
            debug!("CAS: {} already in store, skipping copy", dest.display());
            return Ok(dest);
        }

        // Try hard-link first, fall back to copy.
        match std::fs::hard_link(part_path, &dest) {
            Ok(()) => {
                debug!(
                    "CAS: hard-linked {} -> {}",
                    part_path.display(),
                    dest.display()
                );
            }
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                debug!(
                    "CAS: cross-device, copying {} -> {}",
                    part_path.display(),
                    dest.display()
                );
                std::fs::copy(part_path, &dest).map_err(|e| {
                    WrightError::ForgeError(format!("CAS: failed to copy part to store: {}", e))
                })?;
            }
            Err(e) => {
                return Err(WrightError::ForgeError(format!(
                    "CAS: failed to link part to store: {}",
                    e
                )));
            }
        }

        info!("cached {}", name);
        debug!("CAS: {} stored at {}", name, dest.display());
        Ok(dest)
    }

    /// Remove a part from the CAS store.
    #[allow(dead_code)]
    pub fn remove(&self, name: &str, fingerprint: &str) -> Result<()> {
        let filename = Self::store_filename(name, fingerprint);
        let path = self.store_dir.join(&filename);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                WrightError::ForgeError(format!("CAS: failed to remove {}: {}", path.display(), e))
            })?;
        }
        Ok(())
    }
}
