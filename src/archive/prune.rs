use std::collections::{HashMap, HashSet};
use std::path::Path;
use crate::database::ArchiveDb;
use crate::archive::resolver::ResolvedPartVersioned;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};

pub struct PruneReport {
    pub untracked: Vec<std::path::PathBuf>,
    pub stale_tracked: Vec<StaleArchive>,
    pub stale_db_rows: Vec<String>,
}

pub struct StaleArchive {
    pub path: std::path::PathBuf,
    pub name: String,
    pub version: String,
    pub release: u32,
}

pub async fn plan_prune(
    archive_db: &ArchiveDb,
    installed_db: &InstalledDb,
    parts_dir: &Path,
    prune_untracked: bool,
    keep_latest: bool,
) -> Result<PruneReport> {
    let tracked = archive_db.list_parts(None).await?;
    let tracked_filenames: HashSet<String> = tracked.iter().map(|p| p.filename.clone()).collect();

    let mut untracked = Vec::new();
    let mut stale_tracked = Vec::new();

    if prune_untracked {
        let mut entries = tokio::fs::read_dir(parts_dir).await.map_err(WrightError::IoError)?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !tokio::fs::metadata(&path).await.map(|m| m.is_file()).unwrap_or(false) {
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

    if keep_latest {
        let mut keep_filenames: HashSet<String> = HashSet::new();
        let mut latest_by_name: HashMap<&str, &crate::database::ArchivePart> = HashMap::new();
        for part in &tracked {
            let keep = match latest_by_name.get(part.name.as_str()) {
                Some(current) => {
                    let candidate = ResolvedPartVersioned {
                        name: part.name.clone(),
                        version: part.version.clone(),
                        release: part.release as u32,
                        epoch: part.epoch as u32,
                        path: std::path::PathBuf::new(),
                        dependencies: Vec::new(),
                    };
                    let incumbent = ResolvedPartVersioned {
                        name: current.name.clone(),
                        version: current.version.clone(),
                        release: current.release as u32,
                        epoch: current.epoch as u32,
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

        for installed in installed_db.list_parts().await? {
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
                    release: part.release as u32,
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

pub async fn apply_prune(
    archive_db: &ArchiveDb,
    installed_db: &InstalledDb,
    parts_dir: &Path,
    prune_untracked: bool,
    keep_latest: bool,
) -> Result<PruneReport> {
    let stale_db_rows = archive_db.remove_missing_files(parts_dir).await?;
    let mut report = plan_prune(archive_db, installed_db, parts_dir, prune_untracked, keep_latest).await?;
    report.stale_db_rows = stale_db_rows;

    for path in &report.untracked {
        if tokio::fs::metadata(path).await.is_ok() {
            tokio::fs::remove_file(path).await.map_err(WrightError::IoError)?;
        }
    }

    for stale in &report.stale_tracked {
        if tokio::fs::metadata(&stale.path).await.is_ok() {
            tokio::fs::remove_file(&stale.path).await.map_err(WrightError::IoError)?;
        }
        // remove_part is not yet implemented in async ArchiveDb, let's implement it if needed or just use query
        sqlx::query("DELETE FROM parts WHERE name = ? AND version = ? AND release = ?")
            .bind(&stale.name)
            .bind(&stale.version)
            .bind(stale.release)
            .execute(&archive_db.pool).await.map_err(|e| WrightError::DatabaseError(e.to_string()))?;
    }
    Ok(report)
}
