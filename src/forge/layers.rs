use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

use crate::error::{Result, WrightError};

/// Best-effort recursive removal that handles two failure modes plain
/// `remove_dir_all` doesn't:
///
/// 1. **Stale overlay mounts.** A prior crashed run can leave an active
///    `overlay` mount under the path.  The kernel returns `EBUSY` from
///    `rmdir`/`unlinkat` against the mount point.  We enumerate
///    `/proc/self/mounts`, lazy-unmount any mount whose target falls inside
///    `path`, then retry.
///
/// 2. **Transient `EBUSY`.** Sometimes a sibling process is holding a
///    handle that drops momentarily later.  We retry with a short backoff.
pub async fn force_clean_dir(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || force_clean_dir_blocking(&path))
        .await
        .map_err(|e| WrightError::ForgeError(format!("clean join: {}", e)))?
}

fn force_clean_dir_blocking(path: &Path) -> Result<()> {
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..3 {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                let is_busy = e.raw_os_error() == Some(libc::EBUSY);
                last_err = Some(e);
                if !is_busy {
                    break;
                }
                let detached = detach_mounts_under(path);
                debug!(
                    event = "clean.ebusy",
                    path = %path.display(),
                    attempt = attempt + 1,
                    detached = detached,
                    "EBUSY on clean; detached {} stale mount(s) and retrying",
                    detached,
                );
                thread::sleep(Duration::from_millis(100 * (1 << attempt)));
            }
        }
    }
    let e = last_err.expect("loop only exits with last_err set on failure");
    Err(WrightError::ForgeError(format!(
        "failed to clean forge directory {}: {}",
        path.display(),
        e,
    )))
}

/// Detach every mount whose target is `path` or a descendant. Returns
/// the number of mounts detached.
fn detach_mounts_under(path: &Path) -> usize {
    let mounts = match std::fs::read_to_string("/proc/self/mounts") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut targets: Vec<PathBuf> = mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _src = fields.next()?;
            let target = fields.next()?;
            Some(PathBuf::from(target))
        })
        .filter(|t| t == path || t.starts_with(path))
        .collect();
    targets.sort_by_key(|t| std::cmp::Reverse(t.components().count()));

    let mut detached = 0;
    for target in targets {
        match nix::mount::umount2(&target, nix::mount::MntFlags::MNT_DETACH) {
            Ok(()) => {
                detached += 1;
                debug!(event = "clean.umount", target = %target.display(), "Detached stale mount");
            }
            Err(e) => {
                warn!(event = "clean.umount_failed",
                    target = %target.display(),
                    error = %e,
                    "Failed to detach stale mount; continuing",
                );
            }
        }
    }
    detached
}

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
/// Each stage gets a **disposable mount point** under `stages/`.  The stable
/// `target/` entry is a symlink that is atomically repointed to the current
/// stage's mount point.  Because we never remount the same directory, the
/// `EBUSY` stale-state race is eliminated from the hot path.
pub struct LayerManager {
    layers_dir: PathBuf,
    target_dir: PathBuf,
    stages_dir: PathBuf,
    ovl_work_dir: PathBuf,
    current_mount: Option<PathBuf>,
}

impl Drop for LayerManager {
    fn drop(&mut self) {
        self.unmount_overlay();
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
    /// │   └── ...
    /// ├── stages/         (disposable mount points, one per stage)
    /// │   ├── 01-fetch/
    /// │   └── ...
    /// ├── target/ -> stages/<current-stage>/   (stable symlink)
    /// └── .ovl_work/      (OverlayFS internal working directories)
    /// ```
    pub fn new(build_root: &Path) -> Result<Self> {
        let layers_dir = build_root.join("layers");
        let target_dir = build_root.join("target");
        let stages_dir = build_root.join("stages");
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
        std::fs::create_dir_all(&stages_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create stages dir {}: {}",
                stages_dir.display(),
                e
            ))
        })?;

        // Create a dummy directory so that the initial symlink has something
        // valid to point at before the first stage mounts.
        let empty_stage = stages_dir.join(".empty");
        std::fs::create_dir_all(&empty_stage).ok();

        remove_path_if_exists(&target_dir)?;
        std::os::unix::fs::symlink(&empty_stage, &target_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create target symlink {}: {}",
                target_dir.display(),
                e
            ))
        })?;

        Ok(Self {
            layers_dir,
            target_dir,
            stages_dir,
            ovl_work_dir,
            current_mount: None,
        })
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
            debug!(event = "layer.clear", dir = %dir.display(), "Clearing existing layer directory");
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
    /// hard-linked into the directory that `target_dir` currently points to,
    /// with later layers taking precedence.
    pub fn populate_target(&self, completed_stages: &[String]) -> Result<()> {
        let resolved = std::fs::canonicalize(&self.target_dir)
            .unwrap_or_else(|_| self.target_dir.clone());

        let Ok(read_dir) = std::fs::read_dir(&resolved) else { return Ok(()); };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) && !path.is_symlink() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }

        for stage_name in completed_stages {
            let layer_dir = self.layer_dir(stage_name);
            if !layer_dir.exists() {
                continue;
            }
            debug!(event = "layer.hardlink", stage = %stage_name, layer = %layer_dir.display(), "Hard-linking layer into target");
            hard_link_all_sync(&layer_dir, &resolved)?;
        }

        Ok(())
    }

    /// Attempt an OverlayFS mount merging `completed_stages` with
    /// `current_stage` as the writable upper layer.
    ///
    /// The mount point is a **fresh disposable directory** under `stages/`;
    /// it is never reused.  The stable `target/` symlink is atomically
    /// repointed to it.  This eliminates the `EBUSY` race that plagued the
    /// old design, where `target/` itself was reused as the mount point.
    ///
    /// Returns `true` if the mount succeeded, `false` if OverlayFS was
    /// unavailable (e.g. missing `CAP_SYS_ADMIN`).  When false, the caller
    /// should fall back to [`populate_target`].
    pub fn mount_overlay(&mut self, current_stage: &str, completed_stages: &[String]) -> Result<bool> {
        use nix::mount::{MsFlags, mount};

        // --- 1. Allocate a disposable mount point ---
        let stage_point = self.stages_dir.join(layer_dir_name(current_stage));

        // Cold path: a prior crashed run may have left a stale overlay here.
        let _ = force_clean_dir_blocking(&stage_point);
        std::fs::create_dir_all(&stage_point).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create stage mount point {}: {}",
                stage_point.display(),
                e
            ))
        })?;

        // --- 2. Upper layer & workdir ---
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

        // --- 3. Build lowerdir stack ---
        let mut lower_parts: Vec<String> = Vec::new();
        for prev_stage in completed_stages.iter().rev() {
            let prev_dir = self.layer_dir(prev_stage);
            if prev_dir.exists() {
                lower_parts.push(prev_dir.display().to_string());
            }
        }

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

        let lowerdirs_str = if lower_parts.is_empty() {
            "(none)".to_string()
        } else {
            lower_parts.join(":")
        };
        debug!(
            event = "layer.mount",
            stage = %current_stage,
            point = %stage_point.display(),
            lowerdirs = %lowerdirs_str,
            upperdir = %upper_dir.display(),
            "Mounting stage overlay to disposable point"
        );

        // --- 4. Mount (no retry: each point is used exactly once) ---
        let mount_result = mount(
            Some("overlay"),
            &stage_point,
            Some("overlay"),
            MsFlags::empty(),
            Some(opts.as_str()),
        );

        match mount_result {
            Ok(()) => {
                // Atomically rotate the stable symlink.
                let temp_link = self.target_dir.with_extension("tmp");
                std::os::unix::fs::symlink(&stage_point, &temp_link).map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to create target symlink {}: {}",
                        temp_link.display(),
                        e
                    ))
                })?;
                std::fs::rename(&temp_link, &self.target_dir).map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to rotate target symlink to {}: {}",
                        stage_point.display(),
                        e
                    ))
                })?;

                self.current_mount = Some(stage_point);
                Ok(true)
            }
            Err(nix::errno::Errno::EPERM) => {
                warn!(
                    event = "layer.mount_no_cap",
                    "overlay mount needs root (or CAP_SYS_ADMIN); using slower directory-based layering instead"
                );
                Ok(false)
            }
            Err(e) => Err(WrightError::ForgeError(format!(
                "failed to mount overlay at {}: {}",
                stage_point.display(),
                e
            ))),
        }
    }

    /// Unmount the current stage overlay (lazy detach).
    ///
    /// Because the mount point is disposable, we do not need to wait for
    /// the kernel to finish tearing it down; the next stage will use a
    /// completely fresh directory.
    pub fn unmount_overlay(&mut self) {
        if let Some(ref mount) = self.current_mount.take() {
            match nix::mount::umount2(mount, nix::mount::MntFlags::MNT_DETACH) {
                Ok(()) => {
                    debug!(
                        event = "layer.unmount",
                        point = %mount.display(),
                        "Lazy-unmounted stage overlay"
                    );
                }
                Err(nix::errno::Errno::EINVAL) => {} // not mounted
                Err(e) => {
                    debug!(
                        "unmount overlay at {} (non-fatal): {}",
                        mount.display(),
                        e
                    );
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

        let mut all_files: Vec<PathBuf> = Vec::new();
        collect_files_recursive(&self.target_dir, &self.target_dir, &mut all_files)?;

        for target_file in &all_files {
            let rel_path = target_file
                .strip_prefix(&self.target_dir)
                .unwrap_or(target_file);

            let already_present = completed_stages.iter().rev().any(|s| {
                let prev_path = self.layer_dir(s).join(rel_path);
                if !prev_path.exists() {
                    return false;
                }
                files_are_identical(&prev_path, target_file).unwrap_or(false)
            });

            if !already_present {
                let dest = layer_dir.join(rel_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
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
            debug!(event = "layer.clear_failed", dir = %dir.display(), "Clearing failed stage layer");
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                warn!(event = "layer.clear_failed",
                    dir = %dir.display(),
                    error = %e,
                    "Failed to clear failed stage layer"
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
                warn!(event = "layer.workdir_clear_failed",
                    dir = %work_dir.display(),
                    error = %e,
                    "Failed to clear overlay work dir"
                );
            }
        }
    }

    fn work_dir_for_stage(&self, stage: &str) -> PathBuf {
        self.ovl_work_dir.join(layer_dir_name(stage))
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
                        let _ = std::fs::remove_file(&dest_path);
                        let _ = std::os::unix::fs::symlink(&target, &dest_path);
                    }
                }
                Some(ft) if ft.is_dir() => {
                    if !path.is_symlink() {
                        dirs_to_visit.push(path);
                    }
                }
                _ => {
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
