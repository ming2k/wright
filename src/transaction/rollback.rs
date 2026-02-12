use std::collections::HashMap;
use std::path::PathBuf;

/// Track files that have been copied during installation for rollback.
pub struct RollbackState {
    /// Files that were created (need to be removed on rollback)
    created_files: Vec<PathBuf>,
    /// Directories that were created (need to be removed on rollback, reverse order)
    created_dirs: Vec<PathBuf>,
    /// Files that were backed up before overwrite (original path -> backup path)
    backups: HashMap<PathBuf, PathBuf>,
}

impl RollbackState {
    pub fn new() -> Self {
        Self {
            created_files: Vec::new(),
            created_dirs: Vec::new(),
            backups: HashMap::new(),
        }
    }

    pub fn record_file_created(&mut self, path: PathBuf) {
        self.created_files.push(path);
    }

    pub fn record_dir_created(&mut self, path: PathBuf) {
        self.created_dirs.push(path);
    }

    /// Undo all recorded changes.
    pub fn rollback(&self) {
        // Remove created files
        for path in self.created_files.iter().rev() {
            let _ = std::fs::remove_file(path);
        }

        // Restore backups
        for (original, backup) in &self.backups {
            let _ = std::fs::copy(backup, original);
            let _ = std::fs::remove_file(backup);
        }

        // Remove created directories (reverse order so children are removed first)
        for path in self.created_dirs.iter().rev() {
            let _ = std::fs::remove_dir(path);
        }
    }
}
