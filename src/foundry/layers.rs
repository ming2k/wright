use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

use crate::error::{Result, WrightError};

/// Best-effort detach of any overlay mounts under the given path.
///
/// This is a lighter-weight alternative to `force_clean_dir` for use on
/// startup: it unmounts stale overlays without deleting any files.
pub async fn detach_stale_mounts(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        detach_mounts_under(&path);
    })
    .await
    .map_err(|e| WrightError::ForgeError(format!("detach stale mounts join: {e}")))?;
    Ok(())
}

/// Best-effort recursive removal that handles stale overlay mounts.
pub async fn force_clean_dir(path: &Path) -> Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || force_clean_dir_blocking(&path))
        .await
        .map_err(|e| WrightError::ForgeError(format!("clean join: {e}")))?
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
                if !is_busy { break; }
                let detached = detach_mounts_under(path);
                debug!(
                    event = "clean.ebusy",
                    path = %path.display(),
                    attempt = attempt + 1,
                    detached = detached,
                    "EBUSY on clean; detached {detached} stale mount(s) and retrying",
                );
                thread::sleep(Duration::from_millis(100 * (1 << attempt)));
            }
        }
    }
    let e = last_err.expect("loop only exits with last_err set on failure");
    Err(WrightError::ForgeError(format!(
        "failed to clean forge directory {}: {e}",
        path.display(),
    )))
}

pub(crate) fn detach_mounts_under(path: &Path) -> usize {
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

/// Build stage layer indices. Source stages (fetch/verify/extract) are NOT
/// included — they are handled by `Charge` and fed into the forge as the
/// immutable `source_dir` base.
const LAYER_INDICES: &[(&str, &str)] = &[
    ("prepare", "01"),
    ("configure", "02"),
    ("compile", "03"),
    ("check", "04"),
    ("staging", "05"),
];

pub const LAYER_STAGES: &[&str] = &[
    "prepare", "configure", "compile", "check", "staging",
];

pub fn layer_dir_name(stage: &str) -> String {
    for (s, idx) in LAYER_INDICES {
        if *s == stage {
            return format!("{}-{}", idx, s);
        }
    }
    format!("99-{}", stage)
}

pub fn layer_index(stage: &str) -> Option<usize> {
    LAYER_STAGES.iter().position(|&s| s == stage)
}

pub fn canonical_layer_order() -> Vec<String> {
    LAYER_STAGES.iter().map(|&s| s.to_string()).collect()
}

/// Manage the per-stage OverlayFS layers for a single plan build.
///
/// The immutable `source_dir` is always the deepest lowerdir. Build stages
/// layer on top of it via OverlayFS or hard-link fallback.
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
    pub fn new(build_root: &Path) -> Result<Self> {
        let layers_dir = build_root.join("layers");
        let target_dir = build_root.join("target");
        let stages_dir = build_root.join("stages");
        let ovl_work_dir = build_root.join(".ovl_work");

        std::fs::create_dir_all(&layers_dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create layers dir {}: {e}", layers_dir.display()))
        })?;
        std::fs::create_dir_all(&ovl_work_dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create overlay work dir {}: {e}", ovl_work_dir.display()))
        })?;
        std::fs::create_dir_all(&stages_dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create stages dir {}: {e}", stages_dir.display()))
        })?;

        let empty_stage = stages_dir.join(".empty");
        std::fs::create_dir_all(&empty_stage).ok();

        remove_path_if_exists(&target_dir)?;
        std::os::unix::fs::symlink(&empty_stage, &target_dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create target symlink {}: {e}", target_dir.display()))
        })?;

        Ok(Self {
            layers_dir,
            target_dir,
            stages_dir,
            ovl_work_dir,
            current_mount: None,
        })
    }

    pub fn layers_dir(&self) -> &Path {
        &self.layers_dir
    }

    pub fn target_dir(&self) -> &Path {
        &self.target_dir
    }

    pub fn layer_dir(&self, stage: &str) -> PathBuf {
        self.layers_dir.join(layer_dir_name(stage))
    }

    pub fn prepare_upper_layer(&self, stage: &str) -> Result<PathBuf> {
        let dir = self.layer_dir(stage);
        if dir.exists() {
            debug!(event = "layer.clear", dir = %dir.display(), "Clearing existing layer directory");
            std::fs::remove_dir_all(&dir).map_err(|e| {
                WrightError::ForgeError(format!("failed to clear layer dir {}: {e}", dir.display()))
            })?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create layer dir {}: {e}", dir.display()))
        })?;
        let work_dir = self.work_dir_for_stage(stage);
        self.reset_overlay_work_dir(&work_dir)?;
        Ok(dir)
    }

    pub fn populate_target(&self, source_dir: &Path, completed_stages: &[String]) -> Result<()> {
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

        // Source dir is always the first (deepest) lowerdir.
        if source_dir.exists() {
            debug!(event = "layer.hardlink", layer = %source_dir.display(), "Hard-linking source dir into target");
            hard_link_all_sync(source_dir, &resolved)?;
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

    pub fn mount_overlay(
        &mut self,
        current_stage: &str,
        source_dir: &Path,
        completed_stages: &[String],
    ) -> Result<bool> {
        use nix::mount::{MsFlags, mount};

        let stage_point = self.stages_dir.join(layer_dir_name(current_stage));
        let _ = force_clean_dir_blocking(&stage_point);
        std::fs::create_dir_all(&stage_point).map_err(|e| {
            WrightError::ForgeError(format!("failed to create stage mount point {}: {e}", stage_point.display()))
        })?;

        let upper_dir = self.layer_dir(current_stage);
        if !upper_dir.exists() {
            std::fs::create_dir_all(&upper_dir).map_err(|e| {
                WrightError::ForgeError(format!("failed to create upper layer dir {}: {e}", upper_dir.display()))
            })?;
        }
        let ovl_work = self.work_dir_for_stage(current_stage);
        self.reset_overlay_work_dir(&ovl_work)?;

        // Build lowerdir stack: completed stages are upper lowerdirs (leftmost
        // = topmost), source_dir is always the deepest (rightmost).
        let mut lower_parts: Vec<String> = Vec::new();
        for prev_stage in completed_stages.iter().rev() {
            let prev_dir = self.layer_dir(prev_stage);
            if prev_dir.exists() {
                lower_parts.push(prev_dir.display().to_string());
            }
        }
        if source_dir.exists() {
            lower_parts.push(source_dir.display().to_string());
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

        debug!(
            event = "layer.mount",
            stage = %current_stage,
            point = %stage_point.display(),
            lowerdirs = %lower_parts.join(":"),
            upperdir = %upper_dir.display(),
            "Mounting stage overlay to disposable point"
        );

        let mount_result = mount(
            Some("overlay"),
            &stage_point,
            Some("overlay"),
            MsFlags::empty(),
            Some(opts.as_str()),
        );

        match mount_result {
            Ok(()) => {
                let temp_link = self.target_dir.with_extension("tmp");
                std::os::unix::fs::symlink(&stage_point, &temp_link).map_err(|e| {
                    WrightError::ForgeError(format!("failed to create target symlink {}: {e}", temp_link.display()))
                })?;
                std::fs::rename(&temp_link, &self.target_dir).map_err(|e| {
                    WrightError::ForgeError(format!("failed to rotate target symlink to {}: {e}", stage_point.display()))
                })?;
                self.current_mount = Some(stage_point);
                Ok(true)
            }
            Err(nix::errno::Errno::EPERM) => {
                warn!(event = "layer.mount_no_cap", "overlay mount needs root (or CAP_SYS_ADMIN); using slower directory-based layering instead");
                Ok(false)
            }
            Err(e) => Err(WrightError::ForgeError(format!(
                "failed to mount overlay at {}: {e}",
                stage_point.display()
            ))),
        }
    }

    pub fn unmount_overlay(&mut self) {
        if let Some(ref mount) = self.current_mount.take() {
            match nix::mount::umount2(mount, nix::mount::MntFlags::MNT_DETACH) {
                Ok(()) => {
                    debug!(event = "layer.unmount", point = %mount.display(), "Lazy-unmounted stage overlay");
                }
                Err(nix::errno::Errno::EINVAL) => {}
                Err(e) => {
                    debug!("unmount overlay at {} (non-fatal): {e}", mount.display());
                }
            }
        }
    }

    pub fn commit_layer(&self, stage: &str, source_dir: &Path, completed_stages: &[String]) -> Result<()> {
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

            // Check against source_dir and all prior layers.
            let already_present = {
                let source_path = source_dir.join(rel_path);
                if source_path.exists() && files_are_identical(&source_path, target_file).unwrap_or(false) {
                    true
                } else {
                    completed_stages.iter().rev().any(|s| {
                        let prev_path = self.layer_dir(s).join(rel_path);
                        prev_path.exists() && files_are_identical(&prev_path, target_file).unwrap_or(false)
                    })
                }
            };

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

    pub fn clear_layer(&self, stage: &str) {
        let dir = self.layer_dir(stage);
        if dir.exists() {
            debug!(event = "layer.clear_failed", dir = %dir.display(), "Clearing failed stage layer");
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                warn!(event = "layer.clear_failed", dir = %dir.display(), error = %e, "Failed to clear failed stage layer");
            }
        }
    }

    pub fn clear_layers_from(&self, from_stage: &str) {
        let from_idx = layer_index(from_stage).unwrap_or(0);
        for &stage in &LAYER_STAGES[from_idx..] {
            self.clear_layer(stage);
            let work_dir = self.work_dir_for_stage(stage);
            if let Err(e) = remove_path_if_exists(&work_dir) {
                warn!(event = "layer.workdir_clear_failed", dir = %work_dir.display(), error = %e, "Failed to clear overlay work dir");
            }
        }
    }

    fn work_dir_for_stage(&self, stage: &str) -> PathBuf {
        self.ovl_work_dir.join(layer_dir_name(stage))
    }

    fn reset_overlay_work_dir(&self, work_dir: &Path) -> Result<()> {
        remove_path_if_exists(work_dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to clear overlay work dir {}: {e}", work_dir.display()))
        })?;
        std::fs::create_dir_all(work_dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create overlay work dir {}: {e}", work_dir.display()))
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

fn hard_link_all_sync(src_dir: &Path, dest_dir: &Path) -> Result<()> {
    let mut dirs_to_visit = vec![src_dir.to_path_buf()];
    while let Some(dir) = dirs_to_visit.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(rel_path) = path.strip_prefix(src_dir) else { continue; };
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
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(()); };
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
        if na == 0 { break; }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_dir_name() {
        assert_eq!(layer_dir_name("prepare"), "01-prepare");
        assert_eq!(layer_dir_name("staging"), "05-staging");
        assert_eq!(layer_dir_name("unknown"), "99-unknown");
    }

    #[test]
    fn test_layer_indices() {
        assert_eq!(layer_index("prepare"), Some(0));
        assert_eq!(layer_index("compile"), Some(2));
        assert_eq!(layer_index("staging"), Some(4));
        assert_eq!(layer_index("unknown"), None);
    }
}
