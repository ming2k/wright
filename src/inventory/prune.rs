use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::database::Database;
use crate::error::{Result, WrightError};
use crate::inventory::db::InventoryDb;
use crate::inventory::resolver::ResolvedPartVersioned;

pub struct PruneReport {
    /// Archives that exist on disk but are not in the inventory DB.
    pub untracked: Vec<std::path::PathBuf>,
    /// Archives that are tracked but are not the latest version (and not currently installed).
    pub stale_tracked: Vec<StaleArchive>,
    /// Inventory DB rows whose files were missing from disk (already cleaned up).
    pub stale_db_rows: Vec<String>,
}

pub struct StaleArchive {
    pub path: std::path::PathBuf,
    pub name: String,
    pub version: String,
    pub release: u32,
}

/// Compute what would be pruned without making any changes.
pub fn plan_prune(
    inventory: &InventoryDb,
    installed_db: &Database,
    parts_dir: &Path,
    prune_untracked: bool,
    keep_latest: bool,
) -> Result<PruneReport> {
    let tracked = inventory.list_parts(None)?;
    let tracked_filenames: HashSet<String> = tracked.iter().map(|p| p.filename.clone()).collect();

    let mut untracked = Vec::new();
    let mut stale_tracked = Vec::new();

    // Collect parts not registered in the inventory DB.
    if prune_untracked {
        let entries = std::fs::read_dir(parts_dir).map_err(WrightError::IoError)?;
        for entry in entries {
            let entry = entry.map_err(WrightError::IoError)?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = match path.file_name().and_then(|s| s.to_str()) {
                Some(f) => f.to_string(),
                None => continue,
            };
            if filename.ends_with(".wright.tar.zst") && !tracked_filenames.contains(&filename) {
                untracked.push(path);
            }
        }
    }

    // Collect old versions that are neither the latest nor currently installed.
    if keep_latest {
        let mut keep_filenames: HashSet<String> = HashSet::new();

        // Keep the latest version of each part name.
        let mut latest_by_name: HashMap<&str, &crate::inventory::db::InventoryPart> =
            HashMap::new();
        for part in &tracked {
            let keep = match latest_by_name.get(part.name.as_str()) {
                Some(current) => {
                    let candidate = ResolvedPartVersioned {
                        name: part.name.clone(),
                        version: part.version.clone(),
                        release: part.release,
                        epoch: part.epoch,
                        path: std::path::PathBuf::new(),
                        dependencies: Vec::new(),
                    };
                    let incumbent = ResolvedPartVersioned {
                        name: current.name.clone(),
                        version: current.version.clone(),
                        release: current.release,
                        epoch: current.epoch,
                        path: std::path::PathBuf::new(),
                        dependencies: Vec::new(),
                    };
                    candidate.version_cmp(&incumbent).is_gt()
                }
                None => true,
            };
            if keep {
                latest_by_name.insert(part.name.as_str(), part);
            }
        }
        for part in latest_by_name.values() {
            keep_filenames.insert(part.filename.clone());
        }

        // Always keep the currently installed version of each part.
        for installed in installed_db.list_parts()? {
            for candidate in &tracked {
                if candidate.name == installed.name
                    && candidate.version == installed.version
                    && candidate.release == installed.release
                    && candidate.epoch == installed.epoch
                {
                    keep_filenames.insert(candidate.filename.clone());
                }
            }
        }

        for part in &tracked {
            if !keep_filenames.contains(&part.filename) {
                stale_tracked.push(StaleArchive {
                    path: parts_dir.join(&part.filename),
                    name: part.name.clone(),
                    version: part.version.clone(),
                    release: part.release,
                });
            }
        }
    }

    Ok(PruneReport {
        untracked,
        stale_tracked,
        stale_db_rows: Vec::new(),
    })
}

/// Remove stale inventory DB rows for files that no longer exist, then apply
/// the prune plan (delete files and deregister tracked entries).
pub fn apply_prune(
    inventory: &InventoryDb,
    installed_db: &Database,
    parts_dir: &Path,
    prune_untracked: bool,
    keep_latest: bool,
) -> Result<PruneReport> {
    // Remove DB rows whose files are gone.
    let stale_db_rows = inventory.remove_missing_files(parts_dir)?;

    let mut report = plan_prune(
        inventory,
        installed_db,
        parts_dir,
        prune_untracked,
        keep_latest,
    )?;
    report.stale_db_rows = stale_db_rows;

    for path in &report.untracked {
        if path.exists() {
            std::fs::remove_file(path).map_err(WrightError::IoError)?;
        }
    }

    for stale in &report.stale_tracked {
        if stale.path.exists() {
            std::fs::remove_file(&stale.path).map_err(WrightError::IoError)?;
        }
        inventory.remove_part(&stale.name, &stale.version, Some(stale.release))?;
    }

    Ok(report)
}
