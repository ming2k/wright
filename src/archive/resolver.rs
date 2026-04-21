use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use crate::config::AssembliesConfig;
use crate::error::{Result, WrightError};
use crate::part::part;
use crate::part::version::Version;

#[inline]
pub fn sanitize_cache_filename(raw: &str) -> String {
    crate::util::sanitize_filename(raw)
}

pub struct LocalResolver {
    pub search_dirs: Vec<PathBuf>,
    pub plans_dirs: Vec<PathBuf>,
    pub assemblies: AssembliesConfig,
    pub archive_db_path: Option<PathBuf>,
}

pub struct ResolvedPart {
    pub name: String,
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

impl Default for LocalResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalResolver {
    pub fn new() -> Self {
        Self {
            search_dirs: Vec::new(),
            plans_dirs: Vec::new(),
            assemblies: AssembliesConfig {
                assemblies: std::collections::HashMap::new(),
            },
            archive_db_path: None,
        }
    }

    pub fn add_search_dir(&mut self, path: PathBuf) {
        self.search_dirs.push(path);
    }

    pub fn add_plans_dir(&mut self, path: PathBuf) {
        self.plans_dirs.push(path);
    }

    pub fn set_archive_db_path(&mut self, path: PathBuf) {
        self.archive_db_path = Some(path);
    }

    pub fn load_assemblies(&mut self, config: AssembliesConfig) {
        self.assemblies = config;
    }

    pub async fn resolve(&self, name: &str) -> Result<Option<ResolvedPart>> {
        self.resolve_local(name).await
    }

    async fn resolve_local(&self, name: &str) -> Result<Option<ResolvedPart>> {
        let archive_db_path = match &self.archive_db_path {
            Some(p) => p,
            None => return Ok(None),
        };
        let archive_db = crate::database::ArchiveDb::open(archive_db_path).await?;
        let entry = match archive_db.find_part(name).await? {
            Some(e) => e,
            None => return Ok(None),
        };
        for dir in &self.search_dirs {
            let path = dir.join(&entry.filename);
            if path.exists() {
                return Ok(Some(ResolvedPart {
                    name: entry.name,
                    path,
                    dependencies: entry.runtime_deps,
                }));
            }
        }
        Ok(None)
    }

    pub async fn resolve_all(&self, name: &str) -> Result<Vec<ResolvedPartVersioned>> {
        let archive_db_path = match &self.archive_db_path {
            Some(p) => p,
            None => return Ok(Vec::new()),
        };
        let archive_db = crate::database::ArchiveDb::open(archive_db_path).await?;
        let entries = archive_db.find_all_versions(name).await?;

        let mut results = Vec::new();
        for entry in entries {
            for dir in &self.search_dirs {
                let path = dir.join(&entry.filename);
                if path.exists() {
                    results.push(ResolvedPartVersioned {
                        name: entry.name.clone(),
                        version: entry.version.clone(),
                        release: entry.release as u32,
                        epoch: entry.epoch as u32,
                        path,
                        dependencies: entry.runtime_deps.clone(),
                    });
                    break;
                }
            }
        }
        Ok(results)
    }

    pub fn read_part(&self, path: &Path) -> Result<ResolvedPart> {
        let partinfo = part::read_partinfo(path)?;
        Ok(ResolvedPart {
            name: partinfo.name,
            path: path.to_path_buf(),
            dependencies: partinfo.runtime_deps,
        })
    }

    pub fn resolve_assembly(&self, assembly_name: &str) -> Result<Vec<PathBuf>> {
        let all_plans = self.get_all_plans()?;
        let mut plan_names = std::collections::HashSet::new();

        if self.assemblies.assemblies.contains_key(assembly_name) {
            self.collect_assembly_members(assembly_name, &mut plan_names);
        }

        let mut paths = Vec::new();
        for name in plan_names {
            if let Some(path) = all_plans.get(&name) {
                paths.push(path.clone());
            } else {
                tracing::warn!("Plan {} defined in assembly {} not found in plans tree", name, assembly_name);
            }
        }
        Ok(paths)
    }

    fn collect_assembly_members(&self, name: &str, members: &mut std::collections::HashSet<String>) {
        if let Some(assembly) = self.assemblies.assemblies.get(name) {
            for plan in &assembly.plans {
                members.insert(plan.clone());
            }
            for include in &assembly.includes {
                self.collect_assembly_members(include, members);
            }
        }
    }

    pub fn get_all_plans(&self) -> Result<std::collections::HashMap<String, PathBuf>> {
        let mut map = std::collections::HashMap::new();
        for root in &self.plans_dirs {
            if !root.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(root) {
                let entry = entry.map_err(|e| WrightError::IoError(std::io::Error::other(e)))?;
                if entry.file_name() == "plan.toml" {
                    if let Ok(manifest) = crate::plan::manifest::PlanManifest::from_file(entry.path()) {
                        map.insert(manifest.plan.name, entry.path().to_path_buf());
                    }
                }
            }
        }
        Ok(map)
    }
}
