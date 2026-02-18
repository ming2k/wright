use std::path::{Path, PathBuf};
use crate::config::{RepoConfig, SourceConfig, AssembliesConfig};
use crate::error::{WrightError, Result};
use crate::package::archive;
use crate::util::download;

/// Strip path separators and dangerous components from a filename derived from a URL.
pub fn sanitize_cache_filename(raw: &str) -> String {
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let name = name.rsplit('\\').next().unwrap_or(name);
    let sanitized: String = name.chars()
        .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
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

pub struct ResolvedPackage {
    pub name: String,
    pub path: PathBuf,
    pub dependencies: Vec<String>,
}

impl SimpleResolver {
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            search_dirs: Vec::new(),
            plans_dirs: Vec::new(),
            remote_sources: Vec::new(),
            cache_dir,
            assemblies: AssembliesConfig { assemblies: std::collections::HashMap::new() },
            download_timeout: 300,
        }
    }

    pub fn load_from_config(&mut self, config: &RepoConfig) {
        for source in &config.source {
            if !source.enabled { continue; }
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
        self.remote_sources.sort_by(|a, b| b.priority.cmp(&a.priority));
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

    pub fn resolve(&self, name: &str) -> Result<Option<ResolvedPackage>> {
        if let Some(pkg) = self.resolve_local(name)? {
            return Ok(Some(pkg));
        }

        if name.starts_with("http") {
             let filename = sanitize_cache_filename(
                 name.split('/').next_back().unwrap_or("package.wright.tar.zst")
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

    fn resolve_local(&self, name: &str) -> Result<Option<ResolvedPackage>> {
        for dir in &self.search_dirs {
            if !dir.exists() { continue; }
            for entry in std::fs::read_dir(dir).map_err(WrightError::IoError)? {
                let entry = entry.map_err(WrightError::IoError)?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("zst") {
                    if let Ok(pkginfo) = archive::read_pkginfo(&path) {
                        if pkginfo.name == name {
                            return Ok(Some(ResolvedPackage {
                                name: pkginfo.name,
                                path,
                                dependencies: pkginfo.runtime_deps,
                            }));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn read_archive(&self, path: &Path) -> Result<ResolvedPackage> {
        let pkginfo = archive::read_pkginfo(path)?;
        Ok(ResolvedPackage {
            name: pkginfo.name,
            path: path.to_path_buf(),
            dependencies: pkginfo.runtime_deps,
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
            if !root.exists() { continue; }
            for entry in walkdir::WalkDir::new(root) {
                let entry = entry.map_err(|e| WrightError::IoError(std::io::Error::other(e)))?;
                if entry.file_name() == "plan.toml" {
                    if let Ok(manifest) = crate::package::manifest::PackageManifest::from_file(entry.path()) {
                        map.insert(manifest.plan.name, entry.path().to_path_buf());
                    }
                }
            }
        }
        Ok(map)
    }
}
