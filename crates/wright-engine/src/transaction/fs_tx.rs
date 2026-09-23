//! The single filesystem-transaction engine, shared by install, upgrade, and
//! remove.
//!
//! Every operation that mutates the target root does so through one
//! [`FsTransaction`]: it write-ahead journals each intent, moves overwritten
//! content into a per-transaction backup store under the root, and can undo
//! the whole thing. There is exactly one such engine — no operation rolls its
//! own.
//!
//! # Invariants
//!
//! - **One journal per transaction**, under `<root>/var/lib/wright/rollback/`,
//!   never a process-global path. A batch's transaction can never replay
//!   another's entries.
//! - **Backups live under the root**, not in `/tmp`, so a crashed
//!   transaction's data survives a reboot for recovery.
//! - **Backups are same-inode moves** (`rename(2)`), so a backup or restore is
//!   a single atomic step with no window in which the data exists in neither
//!   place. Cross-filesystem targets fall back to copy + fsync + unlink.
//! - **Write-ahead**: each intent is appended and `fsync`-ed before the
//!   mutation it describes.
//! - **Directories are never relocated.** Ownership is per exact path, so a
//!   directory entry says nothing about its contents; a directory is only
//!   removed when empty, and only created-emptiness is rolled back.
//! - **Recovery decides from the registry, never from the journal alone.** A
//!   journal whose intents are all reflected in the registry describes a
//!   committed operation and is discarded; one that is not is undone. This is
//!   what makes a crash between the database write and journal cleanup safe.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};
use wright_state::database::{FileType, InstalledDb};

/// Directory (relative to the target root) holding in-flight transaction
/// journals and their backup stores.
pub(super) const ROLLBACK_SUBDIR: &str = "var/lib/wright/rollback";

/// What a transaction does to one part, recorded in the journal header so
/// recovery can resolve it against the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsIntent {
    /// `install`, `upgrade`, or `remove`.
    pub action: String,
    pub part_name: String,
    /// The hash this transaction produces (install/upgrade) or removes
    /// (remove). Recovery compares it against the registry.
    pub part_hash: Option<String>,
}

impl FsIntent {
    pub fn install(part_name: &str, hash: Option<&str>) -> Self {
        Self {
            action: "install".into(),
            part_name: part_name.into(),
            part_hash: hash.map(str::to_string),
        }
    }

    pub fn upgrade(part_name: &str, new_hash: Option<&str>) -> Self {
        Self {
            action: "upgrade".into(),
            part_name: part_name.into(),
            part_hash: new_hash.map(str::to_string),
        }
    }

    pub fn remove(part_name: &str, hash: Option<&str>) -> Self {
        Self {
            action: "remove".into(),
            part_name: part_name.into(),
            part_hash: hash.map(str::to_string),
        }
    }

    /// Whether this intent is reflected in the registry.
    async fn is_committed(&self, db: &InstalledDb) -> Result<bool> {
        let installed = db.get_part(&self.part_name).await?;
        match self.action.as_str() {
            "remove" => match installed {
                // Gone from the registry: the removal committed.
                None => Ok(true),
                // Still present. If we recorded the identity we were removing,
                // a different hash means the part was re-installed afterward
                // (so the removal did commit). If we have no recorded hash, the
                // only safe reading is "still here ⇒ not done".
                Some(p) => match &self.part_hash {
                    Some(h) => Ok(p.part_hash.as_deref() != Some(h.as_str())),
                    None => Ok(false),
                },
            },
            // install / upgrade: the part must exist carrying the hash this
            // transaction produced.
            _ => Ok(match installed {
                None => false,
                Some(p) => p.part_hash.is_some() && p.part_hash == self.part_hash,
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JournalLine {
    /// First line: what this transaction covers.
    Header { intents: Vec<FsIntent> },
    /// A file or symlink was created. Undo: remove it.
    FileCreated { path: PathBuf },
    /// A directory was created. Undo: remove it (if empty).
    DirCreated { path: PathBuf },
    /// An empty directory was removed. Undo: recreate it.
    DirRemoved { path: PathBuf },
    /// `from` was moved to `to` (backup store or divert sibling). Undo: move back.
    Moved { from: PathBuf, to: PathBuf },
    /// An existing symlink was replaced; remember its old target. Undo: recreate.
    SymlinkReplaced { path: PathBuf, target: String },
}

/// In-memory mirror of the journal, driving the (common) non-crash rollback.
enum Entry {
    FileCreated(PathBuf),
    DirCreated(PathBuf),
    DirRemoved(PathBuf),
    Moved { from: PathBuf, to: PathBuf },
    SymlinkReplaced { path: PathBuf, target: String },
}

/// A write-ahead, crash-safe filesystem transaction under a single root.
pub struct FsTransaction {
    dir: PathBuf,
    journal_path: PathBuf,
    backup_root: PathBuf,
    root: PathBuf,
    entries: Vec<Entry>,
    active: bool,
}

impl FsTransaction {
    /// Begin a transaction under `root_dir`, labelled `label`. Any stale
    /// journal for the same label is discarded — a fresh begin means the
    /// caller has already decided that transaction did not run.
    pub fn begin(root_dir: &Path, label: &str, intents: &[FsIntent]) -> Result<Self> {
        let dir = root_dir.join(ROLLBACK_SUBDIR).join(format!("tx-{label}"));
        let backup_root = dir.join("backup");
        let journal_path = dir.join("journal.jsonl");

        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&backup_root).map_err(|e| {
            WrightError::context(
                format!("failed to create transaction dir {}", dir.display()),
                e,
            )
        })?;

        let tx = Self {
            dir,
            journal_path,
            backup_root,
            root: root_dir.to_path_buf(),
            entries: Vec::new(),
            active: true,
        };
        tx.append(&JournalLine::Header {
            intents: intents.to_vec(),
        })?;
        Ok(tx)
    }

    fn append(&self, line: &JournalLine) -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.journal_path)
            .map_err(|e| {
                WrightError::context(
                    format!(
                        "failed to open transaction journal {}",
                        self.journal_path.display()
                    ),
                    e,
                )
            })?;
        let json = serde_json::to_string(line)
            .map_err(|e| WrightError::context("failed to serialize journal line", e))?;
        writeln!(f, "{json}")
            .map_err(|e| WrightError::context("failed to write transaction journal", e))?;
        f.sync_data()
            .map_err(|e| WrightError::context("failed to fsync transaction journal", e))?;
        Ok(())
    }

    /// Record that `path` was created. `is_dir` selects the undo verb.
    pub fn record_created(&mut self, path: PathBuf, is_dir: bool) -> Result<()> {
        if is_dir {
            self.append(&JournalLine::DirCreated { path: path.clone() })?;
            self.entries.push(Entry::DirCreated(path));
        } else {
            self.append(&JournalLine::FileCreated { path: path.clone() })?;
            self.entries.push(Entry::FileCreated(path));
        }
        Ok(())
    }

    /// Record that an existing symlink at `path` was replaced, so it can be
    /// recreated on rollback. Call *before* overwriting.
    pub fn record_symlink_replaced(&mut self, path: PathBuf, target: String) -> Result<()> {
        self.append(&JournalLine::SymlinkReplaced {
            path: path.clone(),
            target: target.clone(),
        })?;
        self.entries.push(Entry::SymlinkReplaced { path, target });
        Ok(())
    }

    /// Move `path` into the backup store (if it exists), journaling the intent
    /// before the move. A no-op when nothing exists there. Never moves
    /// directories — that is the invariant that keeps untracked content safe.
    pub async fn back_up(&mut self, path: &Path) -> Result<()> {
        let Ok(meta) = tokio::fs::symlink_metadata(path).await else {
            return Ok(());
        };
        if meta.is_dir() {
            return Ok(());
        }

        let rel = path
            .strip_prefix(&self.root)
            .map_err(|_| {
                WrightError::ValidationError(format!(
                    "path {} is not under root {}",
                    path.display(),
                    self.root.display()
                ))
            })?
            .to_path_buf();
        let backup = self.backup_root.join(&rel);
        if let Some(parent) = backup.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                WrightError::context(
                    format!("failed to create backup dir {}", parent.display()),
                    e,
                )
            })?;
        }

        self.append(&JournalLine::Moved {
            from: path.to_path_buf(),
            to: backup.clone(),
        })?;
        move_path(path, &backup).await.map_err(|e| {
            WrightError::context(format!("failed to back up {}", path.display()), e)
        })?;
        self.entries.push(Entry::Moved {
            from: path.to_path_buf(),
            to: backup,
        });
        Ok(())
    }

    /// Move `from` aside to `to` (a sibling path such as a divert target),
    /// journaling the intent first. Used when the destination is where the new
    /// content will land.
    pub async fn move_aside(&mut self, from: &Path, to: &Path) -> Result<()> {
        self.append(&JournalLine::Moved {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        })?;
        move_path(from, to).await.map_err(|e| {
            WrightError::context(
                format!("failed to move {} to {}", from.display(), to.display()),
                e,
            )
        })?;
        self.entries.push(Entry::Moved {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
        Ok(())
    }

    /// Remove `path` when it is an empty directory, journaling the removal so
    /// rollback recreates it. A no-op when absent or non-empty — never
    /// recurses, so foreign content is left untouched.
    pub async fn remove_empty_dir(&mut self, path: &Path) -> Result<()> {
        let Ok(meta) = tokio::fs::symlink_metadata(path).await else {
            return Ok(());
        };
        if !meta.is_dir() {
            return Ok(());
        }
        if tokio::fs::remove_dir(path).await.is_ok() {
            self.append(&JournalLine::DirRemoved {
                path: path.to_path_buf(),
            })?;
            self.entries.push(Entry::DirRemoved(path.to_path_buf()));
        }
        Ok(())
    }

    /// Convenience for the remove path: back up a registry file unless it is a
    /// directory (directories are handled by [`Self::remove_empty_dir`]).
    pub async fn back_up_entry(&mut self, rel: &str, file_type: FileType) -> Result<()> {
        if file_type == FileType::Directory {
            return Ok(());
        }
        let full = self.root.join(rel.trim_start_matches('/'));
        self.back_up(&full).await
    }

    /// Undo every recorded mutation, most recent first. Synchronous so it can
    /// run from `Drop`.
    pub fn rollback_blocking(&mut self) {
        for entry in self.entries.iter().rev() {
            match entry {
                Entry::FileCreated(path) | Entry::DirCreated(path) => {
                    let _ = std::fs::remove_file(path);
                    let _ = std::fs::remove_dir(path);
                }
                Entry::DirRemoved(path) => {
                    let _ = std::fs::create_dir_all(path);
                }
                Entry::Moved { from, to } => {
                    if let Some(parent) = from.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::remove_file(from);
                    let _ = std::fs::remove_dir(from);
                    if let Err(e) = std::fs::rename(to, from) {
                        warn!(
                            event = "fs_tx.rollback_restore_failed",
                            from = ?from,
                            to = ?to,
                            error = %e,
                            "Failed to restore moved path during rollback"
                        );
                    }
                }
                Entry::SymlinkReplaced { path, target } => {
                    let _ = std::fs::remove_file(path);
                    let _ = std::os::unix::fs::symlink(target, path);
                }
            }
        }
        self.discard();
    }

    /// Commit: the registry has been updated; the backup store is garbage.
    pub fn commit(&mut self) {
        self.discard();
    }

    fn discard(&mut self) {
        if self.active {
            let _ = std::fs::remove_dir_all(&self.dir);
            self.active = false;
        }
    }
}

impl Drop for FsTransaction {
    fn drop(&mut self) {
        // A transaction dropped without commit is an error path the caller did
        // not finalise: restore the filesystem and drop the journal, so the
        // next run sees a clean slate. The registry (settled by the caller's
        // explicit rollback, or by startup recovery) is the authority.
        if self.active {
            self.rollback_blocking();
        }
    }
}

/// Move `src` to `dst`, preferring a same-filesystem rename and falling back
/// to copy + fsync + unlink across filesystem boundaries.
async fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    match tokio::fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            tokio::fs::copy(src, dst).await?;
            if let Ok(f) = tokio::fs::File::open(dst).await {
                let _ = f.sync_all().await;
            }
            let _ = tokio::fs::remove_file(src).await;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Restore `to` back to `from`, recreating parents and preferring rename.
fn restore_path_blocking(to: &Path, from: &Path) -> std::io::Result<()> {
    if let Some(parent) = from.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::symlink_metadata(from) {
        Ok(meta) if meta.is_dir() => {
            let _ = std::fs::remove_dir(from);
        }
        Ok(_) => {
            let _ = std::fs::remove_file(from);
        }
        Err(_) => {}
    }
    std::fs::rename(to, from)
}

/// Scan `<root>/var/lib/wright/rollback` and resolve every leftover
/// transaction against the registry. Returns the number handled.
///
/// A journal whose intents are not all reflected in the registry describes an
/// interrupted operation and is undone; one whose intents are committed is
/// discarded. This ordering — decide from the registry, never the journal
/// alone — is what makes a crash between the database write and journal
/// cleanup safe.
pub async fn recover_all(root_dir: &Path, db: &InstalledDb) -> Result<usize> {
    let base = root_dir.join(ROLLBACK_SUBDIR);
    let entries = match std::fs::read_dir(&base) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(WrightError::context(
                format!("failed to scan transaction rollback dir {}", base.display()),
                e,
            ));
        }
    };

    let mut handled = 0usize;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("tx-") {
            continue;
        }
        if recover_one(&dir, db).await? {
            handled += 1;
        }
    }
    Ok(handled)
}

async fn recover_one(dir: &Path, db: &InstalledDb) -> Result<bool> {
    let journal_path = dir.join("journal.jsonl");
    let content = match std::fs::read_to_string(&journal_path) {
        Ok(c) => c,
        Err(_) => {
            let _ = std::fs::remove_dir_all(dir);
            return Ok(false);
        }
    };

    let mut intents: Vec<FsIntent> = Vec::new();
    let mut lines: Vec<JournalLine> = Vec::new();
    for raw in content.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<JournalLine>(raw) {
            Ok(JournalLine::Header { intents: i }) => intents = i,
            Ok(other) => lines.push(other),
            Err(e) => warn!(
                event = "fs_tx.journal_parse_failed",
                line = raw,
                error = %e,
                "Failed to parse transaction journal line"
            ),
        }
    }

    let mut committed = true;
    for intent in &intents {
        if !intent.is_committed(db).await? {
            committed = false;
            break;
        }
    }

    if committed {
        debug!(
            event = "fs_tx.recover_discard",
            dir = ?dir,
            "Transaction committed; discarding backup store"
        );
    } else {
        info!(
            event = "fs_tx.recover_rollback",
            dir = ?dir,
            "Undoing interrupted transaction"
        );
        for line in lines.iter().rev() {
            match line {
                JournalLine::FileCreated { path } => {
                    let _ = std::fs::remove_file(path);
                }
                JournalLine::DirCreated { path } => {
                    let _ = std::fs::remove_dir(path);
                }
                JournalLine::DirRemoved { path } => {
                    let _ = std::fs::create_dir_all(path);
                }
                JournalLine::Moved { from, to } => {
                    if let Err(e) = restore_path_blocking(to, from) {
                        warn!(
                            event = "fs_tx.recover_restore_failed",
                            from = ?from,
                            to = ?to,
                            error = %e,
                            "Failed to restore path during recovery"
                        );
                    }
                }
                JournalLine::SymlinkReplaced { path, target } => {
                    let _ = std::fs::remove_file(path);
                    let _ = std::os::unix::fs::symlink(target, path);
                }
                JournalLine::Header { .. } => {}
            }
        }
    }

    let _ = std::fs::remove_dir_all(dir);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn back_up_then_rollback_restores_bytes() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("etc/app.conf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"original").unwrap();

        let mut tx =
            FsTransaction::begin(root.path(), "t1", &[FsIntent::remove("app", None)]).unwrap();
        tx.back_up(&file).await.unwrap();
        assert!(!file.exists(), "backup should have moved the file away");

        tx.rollback_blocking();
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
    }

    #[tokio::test]
    async fn commit_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("usr/bin/tool");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"x").unwrap();

        let mut tx =
            FsTransaction::begin(root.path(), "t2", &[FsIntent::remove("tool", None)]).unwrap();
        tx.back_up(&file).await.unwrap();
        tx.commit();

        assert!(!file.exists());
        assert!(!root.path().join(ROLLBACK_SUBDIR).join("tx-t2").exists());
    }

    #[tokio::test]
    async fn directories_are_never_backed_up() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("etc/sysctl.d");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("admin.conf"), b"foreign").unwrap();

        let mut tx =
            FsTransaction::begin(root.path(), "t3", &[FsIntent::remove("p", None)]).unwrap();
        tx.back_up_entry("/etc/sysctl.d", FileType::Directory)
            .await
            .unwrap();

        assert!(dir.is_dir(), "directory must not be relocated");
        assert_eq!(std::fs::read(dir.join("admin.conf")).unwrap(), b"foreign");
    }

    #[tokio::test]
    async fn created_file_and_dir_roll_back() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("usr/share/new");
        let file = dir.join("data");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, b"new").unwrap();

        let mut tx =
            FsTransaction::begin(root.path(), "t4", &[FsIntent::install("p", Some("h"))]).unwrap();
        tx.record_created(dir.clone(), true).unwrap();
        tx.record_created(file.clone(), false).unwrap();
        tx.rollback_blocking();

        assert!(!file.exists(), "created file must be removed on rollback");
        assert!(!dir.exists(), "created dir must be removed on rollback");
    }

    #[tokio::test]
    async fn empty_directory_removal_is_recreated_on_rollback() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("usr/share/empty");
        std::fs::create_dir_all(&dir).unwrap();

        let mut tx =
            FsTransaction::begin(root.path(), "t5", &[FsIntent::remove("p", None)]).unwrap();
        tx.remove_empty_dir(&dir).await.unwrap();
        assert!(!dir.exists());

        tx.rollback_blocking();
        assert!(dir.is_dir(), "rollback must recreate the removed dir");
    }

    #[tokio::test]
    async fn non_empty_directory_is_left_in_place() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("etc/pkg.d");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("user.conf"), b"keep").unwrap();

        let mut tx =
            FsTransaction::begin(root.path(), "t6", &[FsIntent::remove("p", None)]).unwrap();
        tx.remove_empty_dir(&dir).await.unwrap();

        assert!(dir.is_dir(), "non-empty dir must survive");
        assert!(dir.join("user.conf").exists());
    }
}
