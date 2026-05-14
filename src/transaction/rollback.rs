use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Track files that have been copied during installation for rollback.
pub struct RollbackState {
    /// Files that were created (need to be removed on rollback)
    created_files: Vec<PathBuf>,
    /// Directories that were created (need to be removed on rollback, reverse order)
    created_dirs: Vec<PathBuf>,
    /// Files that were backed up before overwrite (original path -> backup path)
    backups: HashMap<PathBuf, PathBuf>,
    /// Symlinks that were backed up before overwrite (original path -> target)
    symlink_backups: HashMap<PathBuf, String>,
    /// Path to the on-disk journal file (None for in-memory only)
    journal_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
enum JournalEntry {
    FileCreated { path: PathBuf },
    DirCreated { path: PathBuf },
    Backup { original: PathBuf, backup: PathBuf },
    SymlinkBackup { original: PathBuf, target: String },
}

impl Default for RollbackState {
    fn default() -> Self {
        Self::new()
    }
}

impl RollbackState {
    pub fn new() -> Self {
        Self {
            created_files: Vec::new(),
            created_dirs: Vec::new(),
            backups: HashMap::new(),
            symlink_backups: HashMap::new(),
            journal_path: None,
        }
    }

    /// Create a rollback state backed by a journal file.
    /// If a leftover journal exists, replay it first to recover from a previous crash.
    pub fn with_journal(path: PathBuf) -> Self {
        if path.exists() {
            info!(
                event = "rollback.recovering",
                journal_path = ?path,
                "Recovering unfinished filesystem transaction from rollback journal"
            );
            Self::replay_journal(&path);
        }

        Self {
            created_files: Vec::new(),
            created_dirs: Vec::new(),
            backups: HashMap::new(),
            symlink_backups: HashMap::new(),
            journal_path: Some(path),
        }
    }

    fn append_journal(&self, entry: &JournalEntry) {
        if let Some(ref path) = self.journal_path {
            let result = (|| -> std::io::Result<()> {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                let line = serde_json::to_string(entry)?;
                writeln!(f, "{}", line)?;
                f.sync_data()?;
                Ok(())
            })();
            if let Err(e) = result {
                warn!(event = "rollback.journal_write_failed", error = %e, "Failed to write rollback journal");
            }
        }
    }

    pub fn record_file_created(&mut self, path: PathBuf) {
        self.append_journal(&JournalEntry::FileCreated { path: path.clone() });
        self.created_files.push(path);
    }

    pub fn record_dir_created(&mut self, path: PathBuf) {
        self.append_journal(&JournalEntry::DirCreated { path: path.clone() });
        self.created_dirs.push(path);
    }

    pub fn record_backup(&mut self, original: PathBuf, backup: PathBuf) {
        self.append_journal(&JournalEntry::Backup {
            original: original.clone(),
            backup: backup.clone(),
        });
        self.backups.insert(original, backup);
    }

    pub fn record_symlink_backup(&mut self, original: PathBuf, target: String) {
        self.append_journal(&JournalEntry::SymlinkBackup {
            original: original.clone(),
            target: target.clone(),
        });
        self.symlink_backups.insert(original, target);
    }

    /// Delete the journal file, signaling successful completion.
    pub fn commit(&self) {
        if let Some(ref path) = self.journal_path
            && let Err(e) = std::fs::remove_file(path)
            && e.kind() != io::ErrorKind::NotFound
        {
            warn!(event = "rollback.journal_remove_failed", error = %e, "Failed to remove rollback journal");
        }
    }

    /// Undo all recorded changes.
    pub fn rollback(&self) {
        // Remove created files
        for path in self.created_files.iter().rev() {
            if let Err(e) = std::fs::remove_file(path) {
                warn!(event = "rollback.remove_failed", path = ?path, error = %e, "Failed to remove file during rollback");
            }
        }

        // Restore backups
        for (original, backup) in &self.backups {
            let _ = std::fs::remove_file(original);
            if let Err(e) = std::fs::copy(backup, original) {
                warn!(
                    event = "rollback.restore_failed",
                    original = ?original,
                    backup = ?backup,
                    error = %e,
                    "Failed to restore file during rollback"
                );
            }
            if let Err(e) = std::fs::remove_file(backup) {
                warn!(
                    event = "rollback.backup_remove_failed",
                    backup = ?backup,
                    error = %e,
                    "Failed to remove backup during rollback"
                );
            }
        }

        // Restore symlink backups
        for (original, target) in &self.symlink_backups {
            let _ = std::fs::remove_file(original);
            if let Err(e) = std::os::unix::fs::symlink(target, original) {
                warn!(
                    event = "rollback.symlink_restore_failed",
                    original = ?original,
                    target,
                    error = %e,
                    "Failed to restore symlink during rollback"
                );
            }
        }

        // Remove created directories (reverse order so children are removed first)
        for path in self.created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(path);
        }
    }

    /// Replay a journal file in reverse to undo a crashed transaction.
    pub fn replay_journal(path: &Path) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!(event = "rollback.journal_read_failed", error = %e, "Failed to read rollback journal");
                return;
            }
        };

        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut restore_failures: usize = 0;

        // Process in reverse order
        for line in lines.iter().rev() {
            match serde_json::from_str::<JournalEntry>(line) {
                Ok(JournalEntry::FileCreated { path }) => {
                    if let Err(e) = std::fs::remove_file(&path) {
                        warn!(
                            event = "rollback.replay_remove_failed",
                            path = ?path,
                            error = %e,
                            "Failed to remove file during journal replay"
                        );
                    }
                }
                Ok(JournalEntry::DirCreated { path }) => {
                    let _ = std::fs::remove_dir(&path);
                }
                Ok(JournalEntry::Backup { original, backup }) => {
                    let _ = std::fs::remove_file(&original);
                    match std::fs::copy(&backup, &original) {
                        Ok(_) => {
                            let _ = std::fs::remove_file(&backup);
                        }
                        Err(e) => {
                            debug!(
                                event = "rollback.replay_restore_failed",
                                original = ?original,
                                backup = ?backup,
                                error = %e,
                                "Failed to restore file during journal replay"
                            );
                            restore_failures += 1;
                        }
                    }
                }
                Ok(JournalEntry::SymlinkBackup { original, target }) => {
                    let _ = std::fs::remove_file(&original);
                    if let Err(e) = std::os::unix::fs::symlink(&target, &original) {
                        warn!(
                            event = "rollback.replay_symlink_failed",
                            original = ?original,
                            target,
                            error = %e,
                            "Failed to restore symlink during journal replay"
                        );
                    }
                }
                Err(e) => {
                    warn!(event = "rollback.replay_parse_failed", line, error = %e, "Failed to parse journal line during replay");
                }
            }
        }

        if restore_failures > 0 {
            warn!(
                event = "rollback.replay_restore_loss",
                restore_failures, "Files could not be restored during journal replay"
            );
        }

        match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                warn!(event = "rollback.journal_cleanup_failed", error = %e, "Failed to remove replayed journal");
            }
            _ => {
                info!(event = "rollback.cleanup_complete", "Cleanup complete");
                debug!(event = "rollback.journal_removed", journal_path = ?path, "Removed rollback journal");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_roundtrip_with_special_paths() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("test.journal");
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();

        let colon_file = root.join("my:file.txt");
        std::fs::write(&colon_file, b"hello").unwrap();

        {
            let mut state = RollbackState::with_journal(journal.clone());
            state.record_file_created(colon_file.clone());
        }

        assert!(journal.exists());
        let content = std::fs::read_to_string(&journal).unwrap();
        assert!(content.contains("FileCreated"));

        RollbackState::replay_journal(&journal);
        assert!(!colon_file.exists());
        assert!(!journal.exists());
    }

    #[test]
    fn test_journal_backup_with_colons() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("test.journal");

        let original = dir.path().join("a:b:c.txt");
        let backup = dir.path().join("backup:d.txt");
        std::fs::write(&original, b"original").unwrap();
        std::fs::write(&backup, b"backup-content").unwrap();

        std::fs::write(&original, b"tampered").unwrap();

        {
            let mut state = RollbackState::with_journal(journal.clone());
            state.record_backup(original.clone(), backup.clone());
        }

        RollbackState::replay_journal(&journal);
        assert_eq!(
            std::fs::read_to_string(&original).unwrap(),
            "backup-content"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn test_journal_roundtrip_with_escaped_chars() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("test2.journal");
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();

        let weird_file = root.join("my\tfile\nname.txt");
        std::fs::write(&weird_file, b"hello").unwrap();

        {
            let mut state = RollbackState::with_journal(journal.clone());
            state.record_file_created(weird_file.clone());
        }

        RollbackState::replay_journal(&journal);
        assert!(!weird_file.exists());
        assert!(!journal.exists());
    }
}
