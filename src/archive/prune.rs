use crate::archive::resolver::{pick_latest, ResolvedPartVersioned};
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use std::collections::HashMap;
use std::path::Path;

pub struct PruneReport {
    pub stale: Vec<StaleArchive>,
}

pub struct StaleArchive {
    pub path: std::path::PathBuf,
    pub name: String,
    pub version: String,
    pub release: u32,
}

pub async fn plan_prune(
    installed_db: &InstalledDb,
    parts_dir: &Path,
    keep_latest: bool,
) -> Result<PruneReport> {
    let parts_dir = parts_dir.to_path_buf();

    let all_archives = tokio::task::spawn_blocking(move || {
        let mut archives = Vec::new();
        if !parts_dir.exists() {
            return Ok(archives);
        }
        let entries = match std::fs::read_dir(&parts_dir) {
            Ok(e) => e,
            Err(_) => return Ok(archives),
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            let fname = match path.file_name().and_then(|s| s.to_str()) {
                Some(f) => f,
                None => continue,
            };
            if !fname.ends_with(".wright.tar.zst") {
                continue;
            }
            let partinfo = match crate::part::part::read_partinfo(&path) {
                Ok(p) => p,
                Err(_) => continue,
            };
            archives.push(ResolvedPartVersioned {
                name: partinfo.name,
                version: partinfo.version,
                release: partinfo.release,
                epoch: partinfo.epoch,
                path,
                dependencies: partinfo.runtime_deps,
            });
        }
        Ok::<Vec<ResolvedPartVersioned>, WrightError>(archives)
    })
    .await
    .map_err(|e| WrightError::BuildError(format!("prune scan failed: {}", e)))??;

    let mut stale = Vec::new();

    if keep_latest {
        let mut by_name: HashMap<String, Vec<&ResolvedPartVersioned>> = HashMap::new();
        for arc in &all_archives {
            by_name.entry(arc.name.clone()).or_default().push(arc);
        }

        let installed = installed_db.list_parts().await?;
        let installed_set: std::collections::HashSet<(String, String, u32, u32)> = installed
            .into_iter()
            .map(|p| (p.name, p.version, p.release as u32, p.epoch as u32))
            .collect();

        for (_name, versions) in by_name {
            let owned: Vec<ResolvedPartVersioned> = versions.iter().map(|v| (*v).clone()).collect();
            let latest = match pick_latest(&owned) {
                Some(l) => l,
                None => continue,
            };

            for arc in versions {
                // Keep the latest version and installed versions
                if arc.path == latest.path {
                    continue;
                }
                if installed_set.contains(&(
                    arc.name.clone(),
                    arc.version.clone(),
                    arc.release,
                    arc.epoch,
                )) {
                    continue;
                }
                stale.push(StaleArchive {
                    path: arc.path.clone(),
                    name: arc.name.clone(),
                    version: arc.version.clone(),
                    release: arc.release,
                });
            }
        }
    }

    Ok(PruneReport { stale })
}

pub async fn apply_prune(
    installed_db: &InstalledDb,
    parts_dir: &Path,
    keep_latest: bool,
) -> Result<PruneReport> {
    let report = plan_prune(installed_db, parts_dir, keep_latest).await?;

    for stale in &report.stale {
        if tokio::fs::metadata(&stale.path).await.is_ok() {
            tokio::fs::remove_file(&stale.path)
                .await
                .map_err(WrightError::IoError)?;
        }
    }
    Ok(report)
}
