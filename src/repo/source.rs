use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::config::{AssembliesConfig, RepoConfig, SourceConfig};
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::version::Version;
use crate::util::download;

/// Strip path separators and dangerous components from a filename derived from a URL.
pub fn sanitize_cache_filename(raw: &str) -> String {
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let name = name.rsplit('\\').next().unwrap_or(name);
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == '\0' {
                '_'
            } else {
                c
            }
        })
        .collect();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "download".to_string()
    } else {
        sanitized
    }
}

pub struct SimpleResolver {
    pub search_dirs: Vec<PathBuf>,
    pub plans_dirs: Vec<PathBuf>,
    pub remote_sources: Vec<SourceConfig>,
    pub cache_dir: PathBuf,
    pub assemblies: AssembliesConfig,
    pub download_timeout: u64,
}

pub struct ResolvedPart {
    pub name: String,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

/// A resolved part with full version information, for multi-version queries.
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
    /// Compare by (epoch, version, release). Returns ordering relative to `other`.
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

/// From a list of resolved parts, pick the latest by (epoch, version, release).
pub fn pick_latest(parts: &[ResolvedPartVersioned]) -> Option<&ResolvedPartVersioned> {
    parts
        .iter()
        .max_by(|a, b| a.version_cmp(b))
}

/// From a list of resolved parts, find one matching a specific version string.
/// If multiple releases exist for that version, returns the highest release.
pub fn pick_version<'a>(
    parts: &'a [ResolvedPartVersioned],
    version: &str,
) -> Option<&'a ResolvedPartVersioned> {
    parts
        .iter()
        .filter(|p| p.version == version)
        .max_by_key(|p| p.release)
}

impl SimpleResolver {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            search_dirs: Vec::new(),
            plans_dirs: Vec::new(),
            remote_sources: Vec::new(),
            cache_dir,
            assemblies: AssembliesConfig {
                assemblies: std::collections::HashMap::new(),
            },
            download_timeout: 300,
        }
    }

    pub fn load_from_config(&mut self, config: &RepoConfig) {
        for source in &config.source {
            if !source.enabled {
                continue;
            }
            match source.type_.as_str() {
                "local" | "hold" => {
                    if let Some(ref path) = source.path {
                        if source.type_ == "local" {
                            self.add_search_dir(path.clone());
                        } else {
                            self.add_plans_dir(path.clone());
                        }
                    }
                }
                "remote" => {
                    if source.url.is_some() {
                        self.remote_sources.push(source.clone());
                    }
                }
                _ => {}
            }
        }
        self.remote_sources
            .sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    pub fn add_search_dir(&mut self, path: PathBuf) {
        self.search_dirs.push(path);
    }

    pub fn add_plans_dir(&mut self, path: PathBuf) {
        self.plans_dirs.push(path);
    }

    pub fn load_assemblies(&mut self, config: AssembliesConfig) {
        self.assemblies = config;
    }

    pub fn resolve(&self, name: &str) -> Result<Option<ResolvedPart>> {
        if let Some(pkg) = self.resolve_local(name)? {
            return Ok(Some(pkg));
        }

        if name.starts_with("http") {
            let filename = sanitize_cache_filename(
                name.split('/').next_back().unwrap_or("part.wright.tar.zst"),
            );
            std::fs::create_dir_all(&self.cache_dir).map_err(WrightError::IoError)?;
            let dest = self.cache_dir.join(&filename);
            if !dest.exists() {
                download::download_file(name, &dest, self.download_timeout)?;
            }
            return self.read_archive(&dest).map(Some);
        }

        Ok(None)
    }

    fn resolve_local(&self, name: &str) -> Result<Option<ResolvedPart>> {
        for dir in &self.search_dirs {
            if !dir.exists() {
                continue;
            }

            // Try index first (fast path)
            if let Some(index) = crate::repo::index::read_index(dir)? {
                if let Some(entry) = index.parts.iter().find(|e| e.name == name) {
                    let path = dir.join(&entry.filename);
                    if path.exists() {
                        return Ok(Some(ResolvedPart {
                            name: entry.name.clone(),
                            path,
                            dependencies: entry.runtime_deps.clone(),
                        }));
                    }
                }
            }

            // Fallback: scan archives directly
            for entry in std::fs::read_dir(dir).map_err(WrightError::IoError)? {
                let entry = entry.map_err(WrightError::IoError)?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("zst") {
                    if let Ok(partinfo) = archive::read_partinfo(&path) {
                        if partinfo.name == name {
                            return Ok(Some(ResolvedPart {
                                name: partinfo.name,
                                path,
                                dependencies: partinfo.runtime_deps,
                            }));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    /// Resolve all available versions of a part by name across all search dirs.
    pub fn resolve_all(&self, name: &str) -> Result<Vec<ResolvedPartVersioned>> {
        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for dir in &self.search_dirs {
            if !dir.exists() {
                continue;
            }

            // Try index first
            if let Some(index) = crate::repo::index::read_index(dir)? {
                for entry in index.parts.iter().filter(|e| e.name == name) {
                    let key = (entry.version.clone(), entry.release, entry.epoch);
                    if seen.insert(key) {
                        let path = dir.join(&entry.filename);
                        if path.exists() {
                            results.push(ResolvedPartVersioned {
                                name: entry.name.clone(),
                                version: entry.version.clone(),
                                release: entry.release,
                                epoch: entry.epoch,
                                path,
                                dependencies: entry.runtime_deps.clone(),
                            });
                        }
                    }
                }
            }

            // Fallback: scan archives
            for entry in std::fs::read_dir(dir).map_err(WrightError::IoError)? {
                let entry = entry.map_err(WrightError::IoError)?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("zst") {
                    if let Ok(partinfo) = archive::read_partinfo(&path) {
                        if partinfo.name == name {
                            let key = (
                                partinfo.version.clone(),
                                partinfo.release,
                                partinfo.epoch,
                            );
                            if seen.insert(key) {
                                results.push(ResolvedPartVersioned {
                                    name: partinfo.name,
                                    version: partinfo.version,
                                    release: partinfo.release,
                                    epoch: partinfo.epoch,
                                    path,
                                    dependencies: partinfo.runtime_deps,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    pub fn read_archive(&self, path: &Path) -> Result<ResolvedPart> {
        let partinfo = archive::read_partinfo(path)?;
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
                tracing::warn!(
                    "Plan {} defined in assembly {} not found in plans tree",
                    name,
                    assembly_name
                );
            }
        }
        Ok(paths)
    }

    fn collect_assembly_members(
        &self,
        name: &str,
        members: &mut std::collections::HashSet<String>,
    ) {
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
                    if let Ok(manifest) =
                        crate::plan::manifest::PlanManifest::from_file(entry.path())
                    {
                        map.insert(manifest.plan.name, entry.path().to_path_buf());
                    }
                }
            }
        }
        Ok(map)
    }
}
