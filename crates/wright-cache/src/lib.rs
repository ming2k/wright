//! Build artifact reuse cache for Wright (ADR-0048).
//!
//! Provides zero-second compilation reuse by caching sealed `.part` archives
//! keyed by the plan's transitive build closure fingerprint.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

pub mod error;
pub use error::{CacheError, Result};

/// Build artifact reuse cache.
///
/// Keys cached `.part` archives by the SHA-256 fingerprint of the plan's
/// transitive build closure.
pub struct BuildCache {
    store_dir: PathBuf,
}

impl BuildCache {
    pub fn new(store_dir: PathBuf) -> Self {
        Self { store_dir }
    }

    /// Compute the closure fingerprint for a plan given its own build key and
    /// the fingerprints of its direct build dependencies.
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

    /// Compute the cache filename for a part.
    pub fn store_filename(name: &str, fingerprint: &str) -> String {
        format!("{}-{}.part", &fingerprint[..16], name)
    }

    /// Content hash used to verify byte equality.
    fn content_hash(path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Some(format!("{:x}", hasher.finalize()))
    }

    /// Query the cache for a pre-built part matching the given fingerprint.
    pub fn resolve(&self, name: &str, fingerprint: &str) -> Option<PathBuf> {
        let filename = Self::store_filename(name, fingerprint);
        let path = self.store_dir.join(&filename);
        if path.exists() {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > 0 => {
                    debug!(event = "cache.hit", path = %path.display(), size = meta.len(), "cache hit");
                    Some(path)
                }
                Ok(_) => {
                    debug!(event = "cache.miss_empty", path = %path.display(), "cache miss: empty file");
                    None
                }
                Err(_) => None,
            }
        } else {
            debug!(event = "cache.miss", filename = %filename, "cache miss: not in store");
            None
        }
    }

    /// Store a `.wright.tar.zst` part archive in the cache store.
    pub fn store(&self, part_path: &Path, name: &str, fingerprint: &str) -> Result<PathBuf> {
        let filename = Self::store_filename(name, fingerprint);
        let dest = self.store_dir.join(&filename);

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CacheError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        if dest.exists() {
            let same = match (Self::content_hash(&dest), Self::content_hash(part_path)) {
                (Some(existing), Some(incoming)) => existing == incoming,
                _ => true,
            };
            if same {
                debug!(event = "cache.exists", dest = %dest.display(), "cache already exists, skipping copy");
                return Ok(dest);
            }
            warn!(event = "cache.replaced", dest = %dest.display(), src = %part_path.display(), "cache entry content mismatch; replacing with freshly sealed part");
            std::fs::remove_file(&dest).map_err(|e| CacheError::Io {
                path: dest.clone(),
                source: e,
            })?;
        }

        match std::fs::hard_link(part_path, &dest) {
            Ok(()) => {
                debug!(event = "cache.hardlinked", src = %part_path.display(), dest = %dest.display(), "cache hard-linked");
            }
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                debug!(event = "cache.cross_device_copy", src = %part_path.display(), dest = %dest.display(), "cache cross-device, copying");
                std::fs::copy(part_path, &dest).map_err(|e| CacheError::Io {
                    path: dest.clone(),
                    source: e,
                })?;
            }
            Err(e) => {
                return Err(CacheError::Io {
                    path: dest,
                    source: e,
                });
            }
        }

        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        info!(
            event = "cache.stored",
            plan_name = %name,
            size = size,
            fingerprint = %&fingerprint[..16],
            "cached",
        );
        Ok(dest)
    }

    /// Remove a part from the cache.
    pub fn remove(&self, name: &str, fingerprint: &str) -> Result<()> {
        let filename = Self::store_filename(name, fingerprint);
        let path = self.store_dir.join(&filename);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| CacheError::Io {
                path: path.clone(),
                source: e,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_skips_identical_content() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = BuildCache::new(tmp.path().join("store"));
        let src = tmp.path().join("part-a");
        std::fs::write(&src, b"same bytes").unwrap();

        let fp = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let first = cache.store(&src, "demo", fp).unwrap();
        let second = cache.store(&src, "demo", fp).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&second).unwrap(), b"same bytes");
    }

    #[test]
    fn store_replaces_mismatched_content() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = BuildCache::new(tmp.path().join("store"));
        let fp = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        let stale = tmp.path().join("stale");
        std::fs::write(&stale, b"foreign content").unwrap();
        let dest = cache.store(&stale, "demo", fp).unwrap();

        let fresh = tmp.path().join("fresh");
        std::fs::write(&fresh, b"freshly sealed").unwrap();
        let dest2 = cache.store(&fresh, "demo", fp).unwrap();
        assert_eq!(dest, dest2);
        assert_eq!(std::fs::read(&dest2).unwrap(), b"freshly sealed");
    }
}
