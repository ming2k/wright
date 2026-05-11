use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::version::Version;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[inline]
pub fn sanitize_cache_filename(raw: &str) -> String {
    crate::util::sanitize_filename(raw)
}

#[derive(Debug, Clone)]
pub struct LocalPartStore {
    pub search_dirs: Vec<PathBuf>,
}

pub struct ResolvedPart {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPartVersioned {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

impl ResolvedPartVersioned {
    pub fn version_cmp(&self, other: &Self) -> Ordering {
        if self.epoch != other.epoch {
            return self.epoch.cmp(&other.epoch);
        }
        let self_ver = Version::parse(&self.version).ok();
        let other_ver = Version::parse(&other.version).ok();
        match (self_ver, other_ver) {
            (Some(sv), Some(ov)) => {
                let ord = sv.cmp(&ov);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            _ => {
                let ord = self.version.cmp(&other.version);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
        self.release.cmp(&other.release)
    }
}

pub fn pick_latest(parts: &[ResolvedPartVersioned]) -> Option<&ResolvedPartVersioned> {
    parts.iter().max_by(|a, b| a.version_cmp(b))
}

pub fn pick_version<'a>(
    parts: &'a [ResolvedPartVersioned],
    version: &str,
) -> Option<&'a ResolvedPartVersioned> {
    parts
        .iter()
        .filter(|p| p.version == version)
        .max_by_key(|p| p.release)
}

impl Default for LocalPartStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalPartStore {
    pub fn new() -> Self {
        Self {
            search_dirs: Vec::new(),
        }
    }

    pub fn add_search_dir(&mut self, path: PathBuf) {
        self.search_dirs.push(path);
    }

    pub async fn resolve(&self, name: &str) -> Result<Option<ResolvedPart>> {
        self.resolve_local(name).await
    }

    async fn resolve_local(&self, name: &str) -> Result<Option<ResolvedPart>> {
        let all = self.resolve_all(name).await?;
        Ok(pick_latest(&all).map(|p| ResolvedPart {
            name: p.name.clone(),
            version: p.version.clone(),
            path: p.path.clone(),
            dependencies: p.dependencies.clone(),
        }))
    }

    pub async fn resolve_all(&self, name: &str) -> Result<Vec<ResolvedPartVersioned>> {
        let search_dirs = self.search_dirs.clone();
        let name = name.to_string();

        tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            for dir in &search_dirs {
                if !dir.exists() {
                    continue;
                }
                let entries = match std::fs::read_dir(dir) {
                    Ok(e) => e,
                    Err(_) => continue,
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
                    let partinfo = match archive::read_partinfo(&path) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    if partinfo.name != name {
                        continue;
                    }
                    results.push(ResolvedPartVersioned {
                        name: partinfo.name,
                        version: partinfo.plan.version,
                        release: partinfo.plan.release,
                        epoch: partinfo.plan.epoch,
                        path,
                        dependencies: partinfo.runtime_deps,
                    });
                }
            }
            Ok(results)
        })
        .await
        .map_err(|e| WrightError::ForgeError(format!("part store task failed: {}", e)))?
    }

    pub fn read_part(&self, path: &Path) -> Result<ResolvedPart> {
        let partinfo = archive::read_partinfo(path)?;
        Ok(ResolvedPart {
            name: partinfo.name,
            version: partinfo.plan.version,
            path: path.to_path_buf(),
            dependencies: partinfo.runtime_deps,
        })
    }
}
