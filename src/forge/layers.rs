use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

use crate::error::{Result, WrightError};

/// Layer index numbers for the canonical pipeline stage order.
const LAYER_INDICES: &[(&str, &str)] = &[
    ("fetch", "01"),
    ("verify", "02"),
    ("extract", "03"),
    ("prepare", "04"),
    ("configure", "05"),
    ("compile", "06"),
    ("check", "07"),
    ("staging", "08"),
];

/// Full set of stage names that form the layered pipeline.
pub const LAYER_STAGES: &[&str] = &[
    "fetch",
    "verify",
    "extract",
    "prepare",
    "configure",
    "compile",
    "check",
    "staging",
];

/// Produce the filesystem-safe layer directory name for a stage.
pub fn layer_dir_name(stage: &str) -> String {
    for (s, idx) in LAYER_INDICES {
        if *s == stage {
            return format!("{}-{}", idx, s);
        }
    }
    format!("99-{}", stage)
}

/// Return the numeric index (0-based) of a stage in the canonical layer order.
pub fn layer_index(stage: &str) -> Option<usize> {
    LAYER_STAGES.iter().position(|&s| s == stage)
}

/// Return the canonical layer order as Strings.
pub fn canonical_layer_order() -> Vec<String> {
    LAYER_STAGES.iter().map(|&s| s.to_string()).collect()
}

/// Manage the per-stage OverlayFS layers for a single plan build.
///
/// Layers are stacked: layer 0 is the bottom-most read-only layer, layer N
/// is the writable upper layer for stage N.  OverlayFS merges them into
/// `target_dir`.
///
/// The overlay mount is performed inside a mount namespace.  When the
/// caller does not have `CAP_SYS_ADMIN` (e.g. in unprivileged test
/// scenarios), the mount is skipped gracefully and `target_dir` is used
/// as a plain directory populated from previous completed layers.
pub struct LayerManager {
    layers_dir: PathBuf,
    target_dir: PathBuf,
    ovl_work_dir: PathBuf,
}

impl Drop for LayerManager {
    fn drop(&mut self) {
        self.try_unmount();
    }
}

impl LayerManager {
    /// Construct a layer manager rooted at `build_root`.
    ///
    /// Creates the directory tree:
    /// ```text
    /// <build_root>/
    /// ├── layers/
    /// │   ├── 01-fetch/
    /// │   ├── 02-verify/
    /// │   └── ...
    /// ├── target/         (merged view for stage execution)
    /// └── .ovl_work/      (OverlayFS internal working directory)
    /// ```
    pub fn new(build_root: &Path) -> Result<Self> {
        let layers_dir = build_root.join("layers");
        let target_dir = build_root.join("target");
        let ovl_work_dir = build_root.join(".ovl_work");

        std::fs::create_dir_all(&layers_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create layers dir {}: {}",
                layers_dir.display(),
                e
            ))
        })?;
        std::fs::create_dir_all(&ovl_work_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create overlay work dir {}: {}",
                ovl_work_dir.display(),
                e
            ))
        })?;

        let manager = Self {
            layers_dir,
            target_dir,
            ovl_work_dir,
        };

        // Clean stale mount points before starting.  target/ is scratch state;
        // recreating it avoids reusing a path backed by a stale overlay dentry.
        manager.reset_target_dir()?;

        Ok(manager)
    }

    // --- Directory paths ---

    pub fn layers_dir(&self) -> &Path {
        &self.layers_dir
    }

    pub fn target_dir(&self) -> &Path {
        &self.target_dir
    }

    /// Path to the layer directory for a given stage name.
    pub fn layer_dir(&self, stage: &str) -> PathBuf {
        self.layers_dir.join(layer_dir_name(stage))
    }

    // --- Layer preparation ---

    /// Ensure a layer directory exists and is empty (for a fresh upperdir).
    pub fn prepare_upper_layer(&self, stage: &str) -> Result<PathBuf> {
        let dir = self.layer_dir(stage);
        if dir.exists() {
            debug!("Clearing existing layer directory: {}", dir.display());
            std::fs::remove_dir_all(&dir).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to clear layer dir {}: {}",
                    dir.display(),
                    e
                ))
            })?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create layer dir {}: {}",
                dir.display(),
                e
            ))
        })?;
        let work_dir = self.work_dir_for_stage(stage);
        self.reset_overlay_work_dir(&work_dir)?;
        Ok(dir)
    }

    /// Create the fetch layer (01-fetch) directory for hardlinks to global cache.
    pub fn ensure_fetch_layer(&self) -> Result<PathBuf> {
        let dir = self.layer_dir("fetch");
        std::fs::create_dir_all(&dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create fetch layer dir {}: {}",
                dir.display(),
                e
            ))
        })?;
        Ok(dir)
    }

    /// Create the extract layer (03-extract) directory for extracted sources.
    pub fn ensure_extract_layer(&self) -> Result<PathBuf> {
        let dir = self.layer_dir("extract");
        std::fs::create_dir_all(&dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create extract layer dir {}: {}",
                dir.display(),
                e
            ))
        })?;
        Ok(dir)
    }

    // --- Working directory construction ---

    /// Populate `target_dir` with a merged view of all completed layers up to
    /// (but not including) `current_stage`.
    ///
    /// This is the non-OverlayFS fallback: files from prior layers are
    /// hard-linked into `target_dir`, with later layers taking precedence.
    /// The caller then runs the stage script inside `target_dir` directly.
    ///
    /// After the stage, call [`commit_layer`] or [`clear_layer`].
    pub fn populate_target(&self, completed_stages: &[String]) -> Result<()> {
        self.reset_target_dir()?;

        // Hard-link files from each completed layer into target, with later
        // layers overwriting earlier ones.
        for stage_name in completed_stages {
            let layer_dir = self.layer_dir(stage_name);
            if !layer_dir.exists() {
                continue;
            }
            debug!(
                "Hard-linking layer '{}' into target: {}",
                stage_name,
                layer_dir.display()
            );
            hard_link_all_sync(&layer_dir, &self.target_dir)?;
        }

        Ok(())
    }

    /// Attempt an OverlayFS mount merging `completed_stages` with
    /// `current_stage` as the writable upper layer.
    ///
    /// Returns `true` if the mount succeeded, `false` if OverlayFS was
    /// unavailable (e.g. missing `CAP_SYS_ADMIN`).  When false, the caller
    /// should fall back to [`populate_target`].
    pub fn mount_overlay(&self, current_stage: &str, completed_stages: &[String]) -> Result<bool> {
        use nix::mount::{MsFlags, mount};

        let mut lower_parts: Vec<String> = Vec::new();
        for prev_stage in completed_stages {
            let prev_dir = self.layer_dir(prev_stage);
            if prev_dir.exists() {
                lower_parts.push(prev_dir.display().to_string());
            }
        }

        let upper_dir = self.layer_dir(current_stage);
        if !upper_dir.exists() {
            std::fs::create_dir_all(&upper_dir).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to create upper layer dir {}: {}",
                    upper_dir.display(),
                    e
                ))
            })?;
        }

        let ovl_work = self.work_dir_for_stage(current_stage);
        self.reset_overlay_work_dir(&ovl_work)?;

        // Unmount any prior overlay at target and recreate the scratch
        // mountpoint before asking the kernel to attach a new overlay.
        self.reset_target_dir()?;

        let opts = if lower_parts.is_empty() {
            let dummy_lower = self.layers_dir.join(".empty-lower");
            std::fs::create_dir_all(&dummy_lower).ok();
            format!(
                "lowerdir={},upperdir={},workdir={}",
                dummy_lower.display(),
                upper_dir.display(),
                ovl_work.display(),
            )
        } else {
            format!(
                "lowerdir={},upperdir={},workdir={}",
                lower_parts.join(":"),
                upper_dir.display(),
                ovl_work.display(),
            )
        };

        debug!(
            "Mounting stage overlay: lowerdirs=[{}], upperdir={}",
            if lower_parts.is_empty() {
                "(none)".to_string()
            } else {
                lower_parts.join(":")
            },
            upper_dir.display(),
        );

        let mount_result = mount(
            Some("overlay"),
            &self.target_dir,
            Some("overlay"),
            MsFlags::empty(),
            Some(opts.as_str()),
        );

        match mount_result {
            Ok(()) => Ok(true),
            Err(nix::errno::Errno::EPERM) => {
                warn!(
                    "OverlayFS mount requires CAP_SYS_ADMIN; falling back to directory-based layering"
                );
                Ok(false)
            }
            Err(nix::errno::Errno::ESTALE) | Err(nix::errno::Errno::EBUSY) => {
                warn!(
                    "OverlayFS mount at {} returned ESTALE/EBUSY; repairing stale mount state and retrying",
                    self.target_dir.display()
                );
                self.reset_target_dir()?;
                self.reset_overlay_work_dir(&ovl_work)?;
                match mount(
                    Some("overlay"),
                    &self.target_dir,
                    Some("overlay"),
                    MsFlags::empty(),
                    Some(opts.as_str()),
                ) {
                    Ok(()) => Ok(true),
                    Err(nix::errno::Errno::EPERM) => {
                        warn!(
                            "OverlayFS mount requires CAP_SYS_ADMIN; falling back to directory-based layering"
                        );
                        Ok(false)
                    }
                    Err(e) => Err(WrightError::ForgeError(format!(
                        "failed to mount overlay at {} after stale-state repair: {}",
                        self.target_dir.display(),
                        e
                    ))),
                }
            }
            Err(e) => Err(WrightError::ForgeError(format!(
                "failed to mount overlay at {}: {}",
                self.target_dir.display(),
                e
            ))),
        }
    }

    /// Unmount the overlay at `target_dir` if mounted.
    pub fn unmount_overlay(&self) {
        self.try_unmount();
    }

    /// Attempt a lazy detach of the overlay mount, retrying on transient
    /// EBUSY with exponential backoff.  Safe to call even when nothing is
    /// mounted (EINVAL is silently ignored).
    fn try_unmount(&self) {
        for attempt in 0..5 {
            match nix::mount::umount2(&self.target_dir, nix::mount::MntFlags::MNT_DETACH) {
                Ok(()) => {
                    debug!("unmounted overlay at {}", self.target_dir.display());
                    return;
                }
                Err(nix::errno::Errno::EINVAL) => return, // not mounted
                Err(nix::errno::Errno::EBUSY) if attempt < 4 => {
                    debug!(
                        "unmount overlay at {} returned EBUSY, retrying (attempt {}/{})",
                        self.target_dir.display(),
                        attempt + 1,
                        5
                    );
                    thread::sleep(Duration::from_millis(100 * (1 << attempt)));
                }
                Err(e) => {
                    debug!(
                        "unmount overlay at {} (non-fatal): {}",
                        self.target_dir.display(),
                        e
                    );
                    return;
                }
            }
        }
    }

    /// After a successful stage, capture the new/changed files from
    /// `target_dir` into `layers/<stage>/`.  Only files that differ from
    /// previous layers are stored.
    ///
    /// `completed_stages` lists stages that already have frozen layers.
    pub fn commit_layer(&self, stage: &str, completed_stages: &[String]) -> Result<()> {
        let layer_dir = self.layer_dir(stage);
        if !self.target_dir.exists() {
            return Ok(());
        }

        // Collect all files currently in target.
        let mut all_files: Vec<PathBuf> = Vec::new();
        collect_files_recursive(&self.target_dir, &self.target_dir, &mut all_files)?;

        for target_file in &all_files {
            let rel_path = target_file
                .strip_prefix(&self.target_dir)
                .unwrap_or(target_file);

            // Check whether this file already exists with identical content
            // in any previous layer (scan in reverse order for precedence).
            let already_present = completed_stages.iter().rev().any(|s| {
                let prev_path = self.layer_dir(s).join(rel_path);
                if !prev_path.exists() {
                    return false;
                }
                files_are_identical(&prev_path, target_file).unwrap_or(false)
            });

            if !already_present {
                // Write this file into the current stage's layer.
                let dest = layer_dir.join(rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                // Prefer hard-link if possible (same filesystem), fall back to copy.
                if std::fs::hard_link(target_file, &dest).is_err() {
                    let _ = std::fs::copy(target_file, &dest);
                }
            }
        }

        Ok(())
    }

    /// Clear a layer directory after a failed stage.
    pub fn clear_layer(&self, stage: &str) {
        let dir = self.layer_dir(stage);
        if dir.exists() {
            debug!("Clearing failed stage layer: {}", dir.display());
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                warn!(
                    "Failed to clear failed stage layer {}: {}",
                    dir.display(),
                    e
                );
            }
        }
    }

    /// Clear all layers from `from_stage` forward.
    pub fn clear_layers_from(&self, from_stage: &str) {
        let from_idx = layer_index(from_stage).unwrap_or(0);
        for &stage in &LAYER_STAGES[from_idx..] {
            self.clear_layer(stage);
            let work_dir = self.work_dir_for_stage(stage);
            if let Err(e) = remove_path_if_exists(&work_dir) {
                warn!(
                    "Failed to clear overlay work dir {}: {}",
                    work_dir.display(),
                    e
                );
            }
        }
    }

    fn work_dir_for_stage(&self, stage: &str) -> PathBuf {
        self.ovl_work_dir.join(layer_dir_name(stage))
    }

    fn reset_target_dir(&self) -> Result<()> {
        self.try_unmount();

        match remove_path_if_exists(&self.target_dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                return Err(WrightError::ForgeError(format!(
                    "failed to clear target dir {}: {} — a stale overlay mount may be active. \
                     Run `umount {}` or reboot to clear it.",
                    self.target_dir.display(),
                    e,
                    self.target_dir.display(),
                )));
            }
            Err(e) => {
                return Err(WrightError::ForgeError(format!(
                    "failed to clear target dir {}: {}",
                    self.target_dir.display(),
                    e
                )));
            }
        }

        std::fs::create_dir_all(&self.target_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create target dir {}: {}",
                self.target_dir.display(),
                e
            ))
        })?;
        Ok(())
    }

    fn reset_overlay_work_dir(&self, work_dir: &Path) -> Result<()> {
        remove_path_if_exists(work_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to clear overlay work dir {}: {}",
                work_dir.display(),
                e
            ))
        })?;
        std::fs::create_dir_all(work_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create overlay work dir {}: {}",
                work_dir.display(),
                e
            ))
        })?;
        Ok(())
    }
}

fn remove_path_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Recursively hard-link all files from `src_dir` into `dest_dir`.
/// Existing files in `dest_dir` are overwritten (higher layer takes
/// precedence).  Falls back to copy when hard-link fails.
fn hard_link_all_sync(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    let mut dirs_to_visit = vec![src_dir.to_path_buf()];
    while let Some(dir) = dirs_to_visit.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(rel_path) = path.strip_prefix(src_dir) else {
                continue;
            };
            let dest_path = dest_dir.join(rel_path);
            let file_type = entry.file_type().ok();

            if let Some(parent) = dest_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            match file_type {
                Some(ft) if ft.is_symlink() => {
                    if let Ok(target) = std::fs::read_link(&path) {
                        // Remove existing entry at dest to allow symlink overwrite.
                        let _ = std::fs::remove_file(&dest_path);
                        let _ = std::os::unix::fs::symlink(&target, &dest_path);
                    }
                }
                Some(ft) if ft.is_dir() => {
                    // Don't follow symlinks to dirs.
                    if !path.is_symlink() {
                        dirs_to_visit.push(path);
                    }
                }
                _ => {
                    // Hard-link, fall back to copy.
                    let _ = std::fs::remove_file(&dest_path);
                    if std::fs::hard_link(&path, &dest_path).is_err() {
                        let _ = std::fs::copy(&path, &dest_path);
                    }
                }
            }
        }
    }
    Ok(())
}

fn collect_files_recursive(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = entry.file_type().ok();
        match ft {
            Some(ft) if ft.is_symlink() => out.push(path),
            Some(ft) if ft.is_dir() && !path.is_symlink() => {
                collect_files_recursive(base, &path, out)?;
            }
            _ => out.push(path),
        }
    }
    Ok(())
}

fn files_are_identical(a: &Path, b: &Path) -> std::io::Result<bool> {
    let meta_a = std::fs::symlink_metadata(a)?;
    let meta_b = std::fs::symlink_metadata(b)?;

    if meta_a.file_type().is_symlink() && meta_b.file_type().is_symlink() {
        return Ok(std::fs::read_link(a)? == std::fs::read_link(b)?);
    }
    if meta_a.len() != meta_b.len() {
        return Ok(false);
    }

    use std::io::Read;
    let mut fa = std::fs::File::open(a)?;
    let mut fb = std::fs::File::open(b)?;
    let mut buf_a = [0u8; 8192];
    let mut buf_b = [0u8; 8192];

    loop {
        let na = fa.read(&mut buf_a)?;
        let nb = fb.read(&mut buf_b)?;
        if na != nb || buf_a[..na] != buf_b[..nb] {
            return Ok(false);
        }
        if na == 0 {
            break;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_dir_name() {
        assert_eq!(layer_dir_name("fetch"), "01-fetch");
        assert_eq!(layer_dir_name("staging"), "08-staging");
        assert_eq!(layer_dir_name("unknown"), "99-unknown");
    }

    #[test]
    fn test_layer_indices() {
        assert_eq!(layer_index("fetch"), Some(0));
        assert_eq!(layer_index("compile"), Some(5));
        assert_eq!(layer_index("staging"), Some(7));
        assert_eq!(layer_index("unknown"), None);
    }
}
