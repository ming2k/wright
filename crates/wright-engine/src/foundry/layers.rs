use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

use crate::error::{Result, WrightError};

/// Best-effort detach of any overlay mounts under the given path.
///
/// This is a lighter-weight alternative to `force_clean_dir` for use on
/// startup: it unmounts stale overlays without deleting any files.  Stale
/// mounts can only come from Wright versions before the merged-base redesign
/// (which mounted stage overlays in the parent mount namespace); current
/// Wright mounts stage overlays inside the sandbox namespace, where they die
/// with the sandbox process.
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
        match remove_tree_force(path) {
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

pub const LAYER_STAGES: &[&str] = &["prepare", "configure", "compile", "check", "staging"];

/// Name of the tombstone manifest inside a fallback-mode layer directory
/// listing paths (relative to the build tree root) that the stage deleted.
const LAYER_DELETIONS_FILE: &str = ".wright-layer-deletions";

const BASE_MANIFEST_NAME: &str = ".base_manifest";
const BASE_MANIFEST_FORMAT: &str = "wright-base-v1";

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
/// # Merged-base design
///
/// Each stage's writes are captured in `layers/NN-stage/` (the OverlayFS
/// `upperdir` while the stage runs).  After every stage, that delta is merged
/// into `base/` — a hard-link union of `source_dir` and every completed stage
/// layer.  The next stage's overlay then uses `base/` as its *only*
/// `lowerdir`.
///
/// `base/` is never the `upperdir` or `workdir` of any overlay mount, so the
/// kernel's in-use upperdir protection (`EBUSY` when a lowerdir is an in-use
/// upperdir of a still-dying overlay) can never fire on it.  Stage overlays
/// are mounted inside the sandbox's own mount namespace (see
/// `isolation::StageOverlay`), so mounts die with the sandbox and can never
/// leak into the parent mount table.
///
/// Crash consistency: `.base_manifest` records exactly what has been merged
/// into `base/`.  At forge start it is compared against the checkpointed set
/// of completed stages; any mismatch (crash mid-merge, rewound layers,
/// tampering) triggers a full rebuild from `source_dir` + the surviving
/// layers.
pub struct LayerManager {
    layers_dir: PathBuf,
    base_dir: PathBuf,
    target_dir: PathBuf,
    ovl_work_dir: PathBuf,
    base_manifest_path: PathBuf,
}

impl LayerManager {
    pub fn new(build_root: &Path) -> Result<Self> {
        let layers_dir = build_root.join("layers");
        let base_dir = build_root.join("base");
        let target_dir = build_root.join("target");
        let ovl_work_dir = build_root.join(".ovl_work");
        let base_manifest_path = build_root.join(BASE_MANIFEST_NAME);

        std::fs::create_dir_all(&layers_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create layers dir {}: {e}",
                layers_dir.display()
            ))
        })?;
        std::fs::create_dir_all(&base_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create base dir {}: {e}",
                base_dir.display()
            ))
        })?;
        std::fs::create_dir_all(&ovl_work_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create overlay work dir {}: {e}",
                ovl_work_dir.display()
            ))
        })?;

        // `target/` is a real directory (a symlink in pre-merged-base build
        // roots).  In fallback mode it is the stage's working tree; in
        // sandbox mode it is a placeholder path used for logging and scratch
        // path derivation.
        if let Ok(meta) = std::fs::symlink_metadata(&target_dir)
            && !meta.is_dir()
        {
            remove_path_if_exists(&target_dir)?;
        }
        std::fs::create_dir_all(&target_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create target dir {}: {e}",
                target_dir.display()
            ))
        })?;

        Ok(Self {
            layers_dir,
            base_dir,
            target_dir,
            ovl_work_dir,
            base_manifest_path,
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

    /// Build the overlay specification for a stage.  The overlay is mounted
    /// by the sandbox inside its own mount namespace as `/build`; the parent
    /// process never mounts anything.
    pub fn overlay_spec(&self, stage: &str) -> crate::isolation::StageOverlay {
        crate::isolation::StageOverlay {
            lowerdir: self.base_dir.clone(),
            upperdir: self.layer_dir(stage),
            workdir: self.work_dir_for_stage(stage),
        }
    }

    pub fn prepare_upper_layer(&self, stage: &str) -> Result<PathBuf> {
        let dir = self.layer_dir(stage);
        if dir.exists() {
            debug!(event = "layer.clear", dir = %dir.display(), "Clearing existing layer directory");
            remove_tree_force(&dir).map_err(|e| {
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

    /// Give a stage's upper/work dirs fresh *root inodes* while preserving
    /// their contents.
    ///
    /// OverlayFS takes its in-use lock on the upperdir and workdir root
    /// dentries.  When a stage attempt is retried, the previous attempt's
    /// sandbox overlay may still be dying in the kernel, holding the lock on
    /// those inodes; remounting the same dirs would fail with `EBUSY` on
    /// kernels where `index=on` is the default.  Renaming the layer aside
    /// and hard-linking its contents into a brand-new directory yields fresh
    /// root inodes that can never collide with the dying instance, without
    /// discarding the stage's work so far.
    pub fn freshen_upper_layer(&self, stage: &str) -> Result<()> {
        let dir = self.layer_dir(stage);
        if !dir.exists() {
            self.prepare_upper_layer(stage)?;
            return Ok(());
        }
        let tmp = dir.with_extension("freshen");
        remove_path_if_exists(&tmp).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to clear freshen dir {}: {e}",
                tmp.display()
            ))
        })?;
        std::fs::rename(&dir, &tmp).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to set aside layer dir {}: {e}",
                dir.display()
            ))
        })?;
        std::fs::create_dir(&dir).map_err(|e| {
            WrightError::ForgeError(format!("failed to create layer dir {}: {e}", dir.display()))
        })?;
        hard_link_all_sync(&tmp, &dir)?;
        remove_tree_force(&tmp).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to remove freshen dir {}: {e}",
                tmp.display()
            ))
        })?;
        let work_dir = self.work_dir_for_stage(stage);
        self.reset_overlay_work_dir(&work_dir)?;
        Ok(())
    }

    /// Ensure `base/` matches `source_dir` + the given completed stage layers.
    ///
    /// `completed_stages` must be in canonical stage order.  A matching
    /// manifest makes this a no-op; otherwise `base/` is rebuilt from
    /// scratch — an O(tree) hard-link pass that runs only on resume,
    /// rewind, or after a crash interrupted an earlier merge.
    pub fn reconcile_base(&self, source_dir: &Path, completed_stages: &[String]) -> Result<()> {
        let expected = Self::base_manifest_contents(source_dir, completed_stages);
        if std::fs::read_to_string(&self.base_manifest_path).ok().as_deref()
            == Some(expected.as_str())
        {
            debug!(event = "layer.base_reuse", "Merged base up-to-date — reusing base/");
            return Ok(());
        }

        debug!(
            event = "layer.base_rebuild",
            stages = %completed_stages.join(","),
            "Rebuilding merged base from source and completed layers"
        );
        if self.base_dir.exists() {
            remove_tree_force(&self.base_dir).map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to clear base dir {}: {e}",
                    self.base_dir.display()
                ))
            })?;
        }
        std::fs::create_dir_all(&self.base_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create base dir {}: {e}",
                self.base_dir.display()
            ))
        })?;

        if source_dir.exists() {
            hard_link_all_sync(source_dir, &self.base_dir)?;
        }
        for stage in completed_stages {
            let layer = self.layer_dir(stage);
            if layer.exists() {
                merge_layer_tree(&layer, &self.base_dir)?;
            }
        }
        self.write_base_manifest(&expected)
    }

    /// Merge a completed stage's layer into `base/` and record the new
    /// manifest.  `completed_stages` must include `stage` and be in
    /// canonical order — it becomes the manifest content.
    ///
    /// Runs after the stage's sandbox has exited.  The stage's overlay may
    /// still be dying in the kernel (orphan processes releasing the
    /// namespace), but it only references `base/` as a *lowerdir*, which the
    /// kernel never locks — so mutating `base/` here cannot trip the in-use
    /// upperdir protection.
    pub fn merge_layer_into_base(
        &self,
        stage: &str,
        source_dir: &Path,
        completed_stages: &[String],
    ) -> Result<()> {
        let layer = self.layer_dir(stage);
        if layer.exists() {
            merge_layer_tree(&layer, &self.base_dir)?;
        }
        let manifest = Self::base_manifest_contents(source_dir, completed_stages);
        self.write_base_manifest(&manifest)?;
        debug!(event = "layer.base_merge", stage = %stage, "Merged stage layer into base");
        Ok(())
    }

    fn base_manifest_contents(source_dir: &Path, completed_stages: &[String]) -> String {
        use std::fmt::Write as _;
        let mut out = format!("{BASE_MANIFEST_FORMAT}\nsource {}\n", source_dir.display());
        for stage in completed_stages {
            let _ = writeln!(out, "stage {stage}");
        }
        out
    }

    fn write_base_manifest(&self, contents: &str) -> Result<()> {
        let tmp = self.base_manifest_path.with_extension("tmp");
        std::fs::write(&tmp, contents).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to write base manifest {}: {e}",
                tmp.display()
            ))
        })?;
        std::fs::rename(&tmp, &self.base_manifest_path).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to commit base manifest {}: {e}",
                self.base_manifest_path.display()
            ))
        })?;
        Ok(())
    }

    /// Populate `target/` as a real directory tree (fallback mode, when a
    /// stage runs without namespace isolation).  The tree is a hard-link
    /// copy of the merged base.
    pub fn populate_target(&self) -> Result<()> {
        if let Ok(read_dir) = std::fs::read_dir(&self.target_dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) && !path.is_symlink()
                {
                    let _ = remove_tree_force(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        debug!(event = "layer.hardlink", layer = %self.base_dir.display(), "Hard-linking merged base into target");
        hard_link_all_sync(&self.base_dir, &self.target_dir)?;
        Ok(())
    }

    /// Harvest the fallback-mode delta from `target/` into the stage's layer
    /// directory, and record deletions in the layer's tombstone manifest.
    pub fn commit_layer(&self, stage: &str) -> Result<()> {
        let layer_dir = self.layer_dir(stage);
        if !self.target_dir.exists() {
            return Ok(());
        }

        // Additions and modifications: anything in `target/` that is absent
        // from, or differs from, the merged base.
        let mut all_files: Vec<PathBuf> = Vec::new();
        collect_files_recursive(&self.target_dir, &mut all_files)?;
        for target_file in &all_files {
            let rel_path = target_file
                .strip_prefix(&self.target_dir)
                .unwrap_or(target_file);
            let base_path = self.base_dir.join(rel_path);
            let already_present = base_path.exists()
                && files_are_identical(&base_path, target_file).unwrap_or(false);

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

        // Deletions: base entries the stage removed from `target/`.  These
        // become tombstones applied to `base/` at merge time.
        let mut base_files: Vec<PathBuf> = Vec::new();
        collect_files_recursive(&self.base_dir, &mut base_files)?;
        let mut deletions: Vec<String> = Vec::new();
        for base_file in &base_files {
            let rel_path = base_file
                .strip_prefix(&self.base_dir)
                .unwrap_or(base_file);
            if std::fs::symlink_metadata(self.target_dir.join(rel_path)).is_err() {
                deletions.push(rel_path.to_string_lossy().into_owned());
            }
        }
        let tombstone = layer_dir.join(LAYER_DELETIONS_FILE);
        if deletions.is_empty() {
            let _ = std::fs::remove_file(&tombstone);
        } else {
            std::fs::write(&tombstone, deletions.join("\n") + "\n").map_err(|e| {
                WrightError::ForgeError(format!(
                    "failed to write layer deletions {}: {e}",
                    tombstone.display()
                ))
            })?;
        }

        Ok(())
    }

    pub fn clear_layer(&self, stage: &str) {
        let dir = self.layer_dir(stage);
        if dir.exists() {
            debug!(event = "layer.clear", dir = %dir.display(), "Clearing failed stage layer");
            if let Err(e) = remove_tree_force(&dir) {
                warn!(event = "layer.clear_failed", dir = %dir.display(), error = %e, "Failed to clear failed stage layer");
            }
        }
        // The base no longer reflects the surviving layers; force the next
        // reconcile to rebuild it.
        let _ = std::fs::remove_file(&self.base_manifest_path);
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
            WrightError::ForgeError(format!(
                "failed to clear overlay work dir {}: {e}",
                work_dir.display()
            ))
        })?;
        std::fs::create_dir_all(work_dir).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to create overlay work dir {}: {e}",
                work_dir.display()
            ))
        })?;
        Ok(())
    }
}

fn remove_path_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => remove_tree_force(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Recursive removal that tolerates permission-restricted directories.
///
/// The kernel creates OverlayFS workdir internals (`work/`) with mode 000.
/// That is no obstacle for root, but an unprivileged owner cannot traverse
/// such a directory, so plain `remove_dir_all` fails with `EACCES`.  Restore
/// owner rwx on every directory in the tree first, then remove it.
fn remove_tree_force(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        if meta.permissions().mode() & 0o700 != 0o700 {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o700);
            let _ = std::fs::set_permissions(&dir, perms);
        }
        if let Ok(read_dir) = std::fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                stack.push(entry.path());
            }
        }
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Merge a stage layer's delta tree into the merged base.
///
/// Handles the three OverlayFS delta encodings so that `base/` always
/// reflects what the stage's merged view looked like:
///
/// * **Whiteouts** (deletions through the overlay): char device `0:0`
///   (privileged mounts) or a zero-length regular file carrying a
///   `trusted.overlay.whiteout` / `user.overlay.whiteout` xattr
///   (user-namespace mounts).  The corresponding `base/` path is removed.
/// * **Opaque directories** (`trusted.overlay.opaque=y` /
///   `user.overlay.opaque=y`): the `base/` directory is replaced wholesale
///   before the layer's contents are merged in.
/// * **Tombstones** (`LAYER_DELETIONS_FILE`, produced by fallback-mode
///   `commit_layer`): plain-text relative paths removed from `base/`.
///
/// Everything else is hard-linked over (or copied, on cross-device or
/// hard-link failure) with the layer's version shadowing the base's.
fn merge_layer_tree(layer_dir: &Path, base_dir: &Path) -> Result<()> {
    let deletions_file = layer_dir.join(LAYER_DELETIONS_FILE);
    if let Ok(raw) = std::fs::read_to_string(&deletions_file) {
        for line in raw.lines() {
            let rel = line.trim();
            if rel.is_empty() {
                continue;
            }
            remove_base_path(base_dir, Path::new(rel))?;
        }
    }

    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(layer_dir.to_path_buf(), PathBuf::new())];
    while let Some((src, rel)) = stack.pop() {
        let entries = match std::fs::read_dir(&src) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = rel.join(entry.file_name());
            if rel == Path::new(LAYER_DELETIONS_FILE) {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let dest = base_dir.join(&rel);

            if is_overlay_whiteout(&path, &meta) {
                remove_base_path(base_dir, &rel)?;
                continue;
            }

            if meta.is_dir() {
                if is_overlay_opaque(&path, &meta) && dest.exists() {
                    std::fs::remove_dir_all(&dest).map_err(|e| {
                        WrightError::ForgeError(format!(
                            "failed to replace opaque dir {}: {e}",
                            dest.display()
                        ))
                    })?;
                }
                ensure_dest_dir(&dest)?;
                stack.push((path, rel));
                continue;
            }

            // Regular file or symlink: shadow whatever the base has.
            remove_dest_any(&dest)?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to read symlink {}: {e}",
                        path.display()
                    ))
                })?;
                std::os::unix::fs::symlink(&target, &dest).map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to create symlink {}: {e}",
                        dest.display()
                    ))
                })?;
            } else if std::fs::hard_link(&path, &dest).is_err() {
                std::fs::copy(&path, &dest).map_err(|e| {
                    WrightError::ForgeError(format!(
                        "failed to copy {} to {}: {e}",
                        path.display(),
                        dest.display()
                    ))
                })?;
            }
        }
    }
    Ok(())
}

/// Remove `base_dir.join(rel)` whatever its type, ignoring missing paths.
/// Refuses relative paths that could escape the base directory.
fn remove_base_path(base_dir: &Path, rel: &Path) -> Result<()> {
    let is_safe = rel.components().all(|c| {
        matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if !is_safe || rel.as_os_str().is_empty() {
        warn!(event = "layer.base_remove_unsafe", rel = %rel.display(), "Ignoring unsafe deletion path");
        return Ok(());
    }
    let dest = base_dir.join(rel);
    remove_dest_any(&dest)
}

fn remove_dest_any(dest: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            remove_tree_force(dest).map_err(|e| {
                WrightError::ForgeError(format!("failed to remove dir {}: {e}", dest.display()))
            })
        }
        Ok(_) => std::fs::remove_file(dest).map_err(|e| {
            WrightError::ForgeError(format!("failed to remove file {}: {e}", dest.display()))
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(WrightError::ForgeError(format!(
            "failed to inspect {}: {e}",
            dest.display()
        ))),
    }
}

fn ensure_dest_dir(dest: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => {
            std::fs::remove_file(dest).map_err(|e| {
                WrightError::ForgeError(format!("failed to replace file {}: {e}", dest.display()))
            })?;
            std::fs::create_dir(dest).map_err(|e| {
                WrightError::ForgeError(format!("failed to create dir {}: {e}", dest.display()))
            })
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::create_dir(dest).map_err(|e| {
                WrightError::ForgeError(format!("failed to create dir {}: {e}", dest.display()))
            })
        }
        Err(e) => Err(WrightError::ForgeError(format!(
            "failed to inspect {}: {e}",
            dest.display()
        ))),
    }
}

fn xattr_value(path: &Path, name: &str) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let c_name = std::ffi::CString::new(name).ok()?;
    let size = unsafe { libc::lgetxattr(c_path.as_ptr(), c_name.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let read = unsafe {
        libc::lgetxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if read < 0 {
        return None;
    }
    buf.truncate(read as usize);
    Some(buf)
}

/// Detect an OverlayFS whiteout entry: either a char device with rdev 0
/// (created by privileged overlay mounts) or a zero-length regular file
/// carrying a whiteout xattr (user-namespace mounts).
fn is_overlay_whiteout(path: &Path, meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    if meta.file_type().is_char_device() {
        return meta.rdev() == 0;
    }
    if meta.is_file() && meta.len() == 0 {
        return xattr_value(path, "trusted.overlay.whiteout").is_some()
            || xattr_value(path, "user.overlay.whiteout").is_some();
    }
    false
}

/// Detect an OverlayFS opaque directory marker.
fn is_overlay_opaque(path: &Path, meta: &std::fs::Metadata) -> bool {
    if !meta.is_dir() {
        return false;
    }
    xattr_value(path, "trusted.overlay.opaque").as_deref() == Some(b"y")
        || xattr_value(path, "user.overlay.opaque").as_deref() == Some(b"y")
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

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ft = entry.file_type().ok();
        match ft {
            Some(ft) if ft.is_symlink() => out.push(path),
            Some(ft) if ft.is_dir() && !path.is_symlink() => {
                collect_files_recursive(&path, out)?;
            }
            _ => out.push(path),
        }
    }
    Ok(())
}

fn files_are_identical(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let meta_a = std::fs::symlink_metadata(a)?;
    let meta_b = std::fs::symlink_metadata(b)?;

    if meta_a.file_type().is_symlink() && meta_b.file_type().is_symlink() {
        return Ok(std::fs::read_link(a)? == std::fs::read_link(b)?);
    }

    // Hard-linked copies of the same file are identical by construction —
    // the common case when comparing a populated tree against the base.
    if meta_a.dev() == meta_b.dev() && meta_a.ino() == meta_b.ino() {
        return Ok(true);
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
    use std::os::unix::fs::MetadataExt;

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

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn set_xattr(path: &Path, name: &str, value: &[u8]) {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let c_name = std::ffi::CString::new(name).unwrap();
        let rc = unsafe {
            libc::lsetxattr(
                c_path.as_ptr(),
                c_name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        assert_eq!(
            rc,
            0,
            "lsetxattr {name} on {} failed: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
    }

    #[test]
    fn merge_shadows_base_and_hardlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let layer = tmp.path().join("layer");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&layer).unwrap();

        write_file(&base.join("keep.txt"), "base");
        write_file(&base.join("shadow.txt"), "old");
        write_file(&layer.join("shadow.txt"), "new");
        write_file(&layer.join("added.txt"), "added");
        std::os::unix::fs::symlink("added.txt", layer.join("link.txt")).unwrap();

        merge_layer_tree(&layer, &base).unwrap();

        assert_eq!(read(&base.join("keep.txt")), "base");
        assert_eq!(read(&base.join("shadow.txt")), "new");
        assert_eq!(read(&base.join("added.txt")), "added");
        assert_eq!(
            std::fs::read_link(base.join("link.txt")).unwrap(),
            PathBuf::from("added.txt")
        );
        // Files merge as hard-links (same inode), not copies.
        let ino_layer = std::fs::metadata(layer.join("added.txt")).unwrap().ino();
        let ino_base = std::fs::metadata(base.join("added.txt")).unwrap().ino();
        assert_eq!(ino_layer, ino_base);
    }

    #[test]
    fn merge_applies_xattr_whiteout() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let layer = tmp.path().join("layer");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&layer).unwrap();

        write_file(&base.join("doomed.txt"), "x");
        write_file(&base.join("sub/inner.txt"), "y");
        write_file(&base.join("survivor.txt"), "z");

        // Whiteout for a file and for a whole directory subtree.
        write_file(&layer.join("doomed.txt"), "");
        set_xattr(&layer.join("doomed.txt"), "user.overlay.whiteout", b"");
        std::fs::create_dir_all(layer.join("sub")).unwrap();
        write_file(&layer.join("sub/inner.txt"), "");
        set_xattr(&layer.join("sub/inner.txt"), "user.overlay.whiteout", b"");

        merge_layer_tree(&layer, &base).unwrap();

        assert!(!base.join("doomed.txt").exists());
        assert!(!base.join("sub/inner.txt").exists());
        assert_eq!(read(&base.join("survivor.txt")), "z");
        // The surviving base directory must not gain marker files.
        assert!(base.join("sub").exists());
    }

    #[test]
    fn merge_applies_char_device_whiteout_when_permitted() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let layer = tmp.path().join("layer");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&layer).unwrap();

        write_file(&base.join("dev_gone.txt"), "x");
        let marker = layer.join("dev_gone.txt");
        let c_marker = std::ffi::CString::new(marker.as_os_str().as_bytes()).unwrap();
        // Privileged overlay mounts encode whiteouts as char device 0:0.
        let rc = unsafe {
            libc::mknod(
                c_marker.as_ptr(),
                libc::S_IFCHR | 0o644,
                libc::makedev(0, 0),
            )
        };
        if rc != 0 {
            eprintln!(
                "mknod not permitted ({}) — skipping char-device whiteout test",
                std::io::Error::last_os_error()
            );
            return;
        }

        merge_layer_tree(&layer, &base).unwrap();
        assert!(!base.join("dev_gone.txt").exists());
    }

    #[test]
    fn merge_applies_tombstones() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let layer = tmp.path().join("layer");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&layer).unwrap();

        write_file(&base.join("gone.txt"), "x");
        write_file(&base.join("dir/inner.txt"), "y");
        write_file(&base.join("stay.txt"), "z");
        write_file(
            &layer.join(LAYER_DELETIONS_FILE),
            "gone.txt\ndir\n../escape\n\n",
        );

        merge_layer_tree(&layer, &base).unwrap();

        assert!(!base.join("gone.txt").exists());
        assert!(!base.join("dir").exists());
        assert_eq!(read(&base.join("stay.txt")), "z");
        // The tombstone manifest itself is never merged, and traversal
        // entries are refused.
        assert!(!base.join(LAYER_DELETIONS_FILE).exists());
        assert!(!tmp.path().join("escape").exists());
    }

    #[test]
    fn merge_replaces_opaque_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        let layer = tmp.path().join("layer");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&layer).unwrap();

        write_file(&base.join("o/legacy.txt"), "x");
        write_file(&base.join("o/deep/nested.txt"), "y");
        std::fs::create_dir_all(layer.join("o")).unwrap();
        set_xattr(&layer.join("o"), "user.overlay.opaque", b"y");
        write_file(&layer.join("o/fresh.txt"), "z");

        merge_layer_tree(&layer, &base).unwrap();

        assert!(!base.join("o/legacy.txt").exists());
        assert!(!base.join("o/deep").exists());
        assert_eq!(read(&base.join("o/fresh.txt")), "z");
    }

    #[test]
    fn reconcile_builds_reuses_and_rewinds_base() {
        let tmp = tempfile::tempdir().unwrap();
        let build_root = tmp.path().join("workshop/pkg-1.0");
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        write_file(&source.join("s.txt"), "src");

        let mgr = LayerManager::new(&build_root).unwrap();
        write_file(
            &build_root.join("layers/01-prepare/p.txt"),
            "prep",
        );

        // Initial build: source + completed layer.
        let completed = vec!["prepare".to_string()];
        mgr.reconcile_base(&source, &completed).unwrap();
        let base = build_root.join("base");
        assert_eq!(read(&base.join("s.txt")), "src");
        assert_eq!(read(&base.join("p.txt")), "prep");

        // Matching manifest: reconcile is a no-op (does not rebuild).
        std::fs::remove_file(base.join("s.txt")).unwrap();
        write_file(&base.join("s.txt"), "corrupted");
        mgr.reconcile_base(&source, &completed).unwrap();
        assert_eq!(read(&base.join("s.txt")), "corrupted");

        // Dropped manifest forces a rebuild that restores consistency.
        std::fs::remove_file(build_root.join(BASE_MANIFEST_NAME)).unwrap();
        mgr.reconcile_base(&source, &completed).unwrap();
        assert_eq!(read(&base.join("s.txt")), "src");

        // Rewind: completed set shrinks, base loses the layer's content.
        mgr.reconcile_base(&source, &[]).unwrap();
        assert_eq!(read(&base.join("s.txt")), "src");
        assert!(!base.join("p.txt").exists());
    }

    #[test]
    fn commit_and_merge_fallback_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let build_root = tmp.path().join("workshop/pkg-1.0");
        let source = tmp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        write_file(&source.join("a.txt"), "alpha");
        write_file(&source.join("del.txt"), "doomed");

        let mgr = LayerManager::new(&build_root).unwrap();
        mgr.reconcile_base(&source, &[]).unwrap();
        mgr.prepare_upper_layer("configure").unwrap();
        mgr.populate_target().unwrap();

        // Simulate a fallback-mode stage: modify, add, delete.  Real tools
        // replace files rather than writing through shared inodes.
        let target = mgr.target_dir();
        std::fs::remove_file(target.join("a.txt")).unwrap();
        write_file(&target.join("a.txt"), "changed");
        write_file(&target.join("new.txt"), "new");
        std::fs::remove_file(target.join("del.txt")).unwrap();

        mgr.commit_layer("configure").unwrap();
        let layer = mgr.layer_dir("configure");
        assert_eq!(read(&layer.join("a.txt")), "changed");
        assert_eq!(read(&layer.join("new.txt")), "new");
        assert!(!layer.join("del.txt").exists());
        assert_eq!(
            read(&layer.join(LAYER_DELETIONS_FILE)).trim(),
            "del.txt"
        );

        let completed = vec!["configure".to_string()];
        mgr.merge_layer_into_base("configure", &source, &completed)
            .unwrap();
        let base = build_root.join("base");
        assert_eq!(read(&base.join("a.txt")), "changed");
        assert_eq!(read(&base.join("new.txt")), "new");
        assert!(!base.join("del.txt").exists());

        // A subsequent resume reconcile reaches the same state.
        std::fs::remove_file(build_root.join(BASE_MANIFEST_NAME)).unwrap();
        mgr.reconcile_base(&source, &completed).unwrap();
        assert_eq!(read(&base.join("a.txt")), "changed");
        assert!(!base.join("del.txt").exists());
    }

    #[test]
    fn identical_files_fastpath_hardlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        write_file(&a, "same");
        std::fs::hard_link(&a, &b).unwrap();
        assert!(files_are_identical(&a, &b).unwrap());
        std::fs::remove_file(&b).unwrap();
        write_file(&b, "same");
        assert!(files_are_identical(&a, &b).unwrap());
        write_file(&b, "different");
        assert!(!files_are_identical(&a, &b).unwrap());
    }

    #[test]
    fn freshen_preserves_content_with_fresh_root_inode() {
        let tmp = tempfile::tempdir().unwrap();
        let build_root = tmp.path().join("workshop/pkg-1.0");
        let mgr = LayerManager::new(&build_root).unwrap();
        mgr.prepare_upper_layer("compile").unwrap();
        let layer = mgr.layer_dir("compile");
        write_file(&layer.join("obj/out.o"), "object");
        let old_root_ino = std::fs::metadata(&layer).unwrap().ino();
        let old_file_ino = std::fs::metadata(layer.join("obj/out.o")).unwrap().ino();

        mgr.freshen_upper_layer("compile").unwrap();

        let layer = mgr.layer_dir("compile");
        assert_eq!(read(&layer.join("obj/out.o")), "object");
        assert_ne!(std::fs::metadata(&layer).unwrap().ino(), old_root_ino);
        // File contents hard-link back: data inodes are preserved.
        assert_eq!(
            std::fs::metadata(layer.join("obj/out.o")).unwrap().ino(),
            old_file_ino
        );
        // No freshen leftovers and the workdir is reset.
        assert!(!build_root.join("layers/03-compile.freshen").exists());
        assert!(build_root.join(".ovl_work/03-compile").exists());
    }
}
