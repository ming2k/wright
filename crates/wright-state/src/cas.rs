use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};

/// Content-addressed storage for sealed `.part` archives.
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

    /// Content hash used to decide whether an existing store entry really
    /// holds the same bytes as the part being stored.
    fn content_hash(path: &Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Some(format!("{:x}", hasher.finalize()))
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
                    debug!(event = "cas.hit", path = %path.display(), size = meta.len(), "CAS hit");
                    Some(path)
                }
                Ok(_) => {
                    debug!(event = "cas.miss_empty", path = %path.display(), "CAS miss: empty file, discarding");
                    None
                }
                Err(_) => None,
            }
        } else {
            debug!(event = "cas.miss", filename = %filename, "CAS miss: not in store");
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
                WrightError::context(
                    format!("failed to create CAS store directory {}", parent.display()),
                    e,
                )
            })?;
        }

        // If destination already exists with the same content, skip. A
        // same-named entry with different bytes is stale or foreign content
        // (the fingerprint namespace is shared across plans and eras), and
        // must never win over a freshly sealed part — replace it so the
        // store self-heals instead of poisoning every later resolve.
        if dest.exists() {
            let same = match (Self::content_hash(&dest), Self::content_hash(part_path)) {
                (Some(existing), Some(incoming)) => existing == incoming,
                // Unreadable on either side: keep the old behavior of
                // trusting the existing entry rather than risking data loss.
                _ => true,
            };
            if same {
                debug!(event = "cas.exists", dest = %dest.display(), "CAS already exists, skipping copy");
                return Ok(dest);
            }
            warn!(event = "cas.replaced", dest = %dest.display(), src = %part_path.display(), "CAS entry content mismatch; replacing with freshly sealed part");
            std::fs::remove_file(&dest).map_err(|e| {
                WrightError::context(
                    format!("CAS: failed to replace mismatched entry {}", dest.display()),
                    e,
                )
            })?;
        }

        // Try hard-link first, fall back to copy.
        match std::fs::hard_link(part_path, &dest) {
            Ok(()) => {
                debug!(event = "cas.hardlinked", src = %part_path.display(), dest = %dest.display(), "CAS hard-linked");
            }
            Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
                debug!(event = "cas.cross_device_copy", src = %part_path.display(), dest = %dest.display(), "CAS cross-device, copying");
                std::fs::copy(part_path, &dest)
                    .map_err(|e| WrightError::context("CAS: failed to copy part to store", e))?;
            }
            Err(e) => {
                return Err(WrightError::context("CAS: failed to link part to store", e));
            }
        }

        let size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        // Rule B: CAS storage is a transparent side-effect of building;
        // no separate CLI line — the structured log keeps the audit trail.
        info!(
            event = "cas.cached",
            plan_name = %name,
            size = size,
            fingerprint = %&fingerprint[..16],
            "cas stored",
        );
        debug!(event = "cas.stored", name = %name, dest = %dest.display(), "CAS stored");
        Ok(dest)
    }

    /// Remove a part from the CAS store.
    #[allow(dead_code)]
    pub fn remove(&self, name: &str, fingerprint: &str) -> Result<()> {
        let filename = Self::store_filename(name, fingerprint);
        let path = self.store_dir.join(&filename);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                WrightError::context(format!("CAS: failed to remove {}", path.display()), e)
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
        let store = CasStore::new(tmp.path().join("store"));
        let src = tmp.path().join("part-a");
        std::fs::write(&src, b"same bytes").unwrap();

        let fp = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let first = store.store(&src, "demo", fp).unwrap();
        let second = store.store(&src, "demo", fp).unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&second).unwrap(), b"same bytes");
    }

    #[test]
    fn store_replaces_mismatched_content() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CasStore::new(tmp.path().join("store"));
        let fp = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        // Simulate a stale/foreign entry occupying the slot.
        let stale = tmp.path().join("stale");
        std::fs::write(&stale, b"foreign content").unwrap();
        let dest = store.store(&stale, "demo", fp).unwrap();

        // A freshly sealed part with different bytes under the same
        // fingerprint+name must replace the stale entry, not be dropped.
        let fresh = tmp.path().join("fresh");
        std::fs::write(&fresh, b"freshly sealed").unwrap();
        let dest2 = store.store(&fresh, "demo", fp).unwrap();
        assert_eq!(dest, dest2);
        assert_eq!(std::fs::read(&dest2).unwrap(), b"freshly sealed");
    }
}
