use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::warn;

/// Track files that have been copied during installation for rollback.
pub struct RollbackState {
    /// Files that were created (need to be removed on rollback)
    created_files: Vec<PathBuf>,
    /// Directories that were created (need to be removed on rollback, reverse order)
    created_dirs: Vec<PathBuf>,
    /// Files that were backed up before overwrite (original path -> backup path)
    backups: HashMap<PathBuf, PathBuf>,
    /// Path to the on-disk journal file (None for in-memory only)
    journal_path: Option<PathBuf>,
}

// Journal line format uses tab as delimiter (tabs cannot appear in Unix paths).
// FILE_CREATED\t<path>
// DIR_CREATED\t<path>
// BACKUP\t<original>\t<backup>
const DELIM: char = '\t';

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
            journal_path: None,
        }
    }

    /// Create a rollback state backed by a journal file.
    /// If a leftover journal exists, replay it first to recover from a previous crash.
    pub fn with_journal(path: PathBuf) -> Self {
        if path.exists() {
            warn!("Found leftover rollback journal at {}, replaying...", path.display());
            Self::replay_journal(&path);
        }

        Self {
            created_files: Vec::new(),
            created_dirs: Vec::new(),
            backups: HashMap::new(),
            journal_path: Some(path),
        }
    }

    fn append_journal(&self, line: &str) {
        if let Some(ref path) = self.journal_path {
            let result = (|| -> std::io::Result<()> {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                writeln!(f, "{}", line)?;
                f.sync_data()?;
                Ok(())
            })();
            if let Err(e) = result {
                warn!("Failed to write rollback journal: {}", e);
            }
        }
    }

    pub fn record_file_created(&mut self, path: PathBuf) {
        self.append_journal(&format!("FILE_CREATED{}{}", DELIM, path.display()));
        self.created_files.push(path);
    }

    pub fn record_dir_created(&mut self, path: PathBuf) {
        self.append_journal(&format!("DIR_CREATED{}{}", DELIM, path.display()));
        self.created_dirs.push(path);
    }

    pub fn record_backup(&mut self, original: PathBuf, backup: PathBuf) {
        self.append_journal(&format!(
            "BACKUP{}{}{}{}", DELIM, original.display(), DELIM, backup.display()
        ));
        self.backups.insert(original, backup);
    }

    /// Delete the journal file, signaling successful completion.
    pub fn commit(&self) {
        if let Some(ref path) = self.journal_path {
            if let Err(e) = std::fs::remove_file(path) {
                warn!("Failed to remove rollback journal: {}", e);
            }
        }
    }

    /// Undo all recorded changes.
    pub fn rollback(&self) {
        // Remove created files
        for path in self.created_files.iter().rev() {
            if let Err(e) = std::fs::remove_file(path) {
                warn!("Rollback: failed to remove file {}: {}", path.display(), e);
            }
        }

        // Restore backups
        for (original, backup) in &self.backups {
            if let Err(e) = std::fs::copy(backup, original) {
                warn!("Rollback: failed to restore {} from {}: {}", original.display(), backup.display(), e);
            }
            if let Err(e) = std::fs::remove_file(backup) {
                warn!("Rollback: failed to remove backup {}: {}", backup.display(), e);
            }
        }

        // Remove created directories (reverse order so children are removed first)
        for path in self.created_dirs.iter().rev() {
            // remove_dir only removes empty dirs, so failure is expected if dir has other contents
            let _ = std::fs::remove_dir(path);
        }
    }

    /// Replay a journal file in reverse to undo a crashed transaction.
    pub fn replay_journal(path: &Path) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read rollback journal: {}", e);
                return;
            }
        };

        let lines: Vec<&str> = content.lines().collect();

        // Process in reverse order
        for line in lines.iter().rev() {
            let parts: Vec<&str> = line.splitn(3, DELIM).collect();
            match parts.first().copied() {
                Some("FILE_CREATED") if parts.len() == 2 => {
                    if let Err(e) = std::fs::remove_file(parts[1]) {
                        warn!("Journal replay: failed to remove file {}: {}", parts[1], e);
                    }
                }
                Some("DIR_CREATED") if parts.len() == 2 => {
                    let _ = std::fs::remove_dir(parts[1]);
                }
                Some("BACKUP") if parts.len() == 3 => {
                    let original = parts[1];
                    let backup = parts[2];
                    if let Err(e) = std::fs::copy(backup, original) {
                        warn!("Journal replay: failed to restore {} from {}: {}", original, backup, e);
                    }
                    if let Err(e) = std::fs::remove_file(backup) {
                        warn!("Journal replay: failed to remove backup {}: {}", backup, e);
                    }
                }
                _ => {
                    warn!("Journal replay: unrecognized line: {}", line);
                }
            }
        }

        if let Err(e) = std::fs::remove_file(path) {
            warn!("Failed to remove replayed journal: {}", e);
        }
        warn!("Rollback journal replayed and removed");
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

        // Create a file with a colon in the name
        let colon_file = root.join("my:file.txt");
        std::fs::write(&colon_file, b"hello").unwrap();

        {
            let mut state = RollbackState::with_journal(journal.clone());
            state.record_file_created(colon_file.clone());
        }

        // Journal should exist and be parseable
        assert!(journal.exists());
        let content = std::fs::read_to_string(&journal).unwrap();
        assert!(content.contains("FILE_CREATED\t"));

        // Replay should remove the file
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

        // Tamper original to simulate upgrade
        std::fs::write(&original, b"tampered").unwrap();

        {
            let mut state = RollbackState::with_journal(journal.clone());
            state.record_backup(original.clone(), backup.clone());
        }

        // Replay should restore original from backup
        RollbackState::replay_journal(&journal);
        assert_eq!(std::fs::read_to_string(&original).unwrap(), "backup-content");
        assert!(!backup.exists());
    }
}
