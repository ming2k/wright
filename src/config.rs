use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use figment::{Figment, providers::{Format, Toml, Env}};

use crate::error::{Result, WrightError};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GlobalConfig {
    #[serde(default = "default_general")]
    pub general: GeneralConfig,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeneralConfig {
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default = "default_plans_dir")]
    pub plans_dir: PathBuf,
    /// Additional plan search directories consulted after `plans_dir`.
    /// Relative paths are resolved against the working directory at runtime.
    #[serde(default)]
    pub extra_plans_dirs: Vec<PathBuf>,
    #[serde(default = "default_parts_dir", alias = "components_dir")]
    pub parts_dir: PathBuf,
    #[serde(default = "default_source_dir")]
    pub source_dir: PathBuf,
    #[serde(default = "default_installed_db_path", alias = "db_path")]
    pub installed_db_path: PathBuf,
    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,
    #[serde(default = "default_executors_dir")]
    pub executors_dir: PathBuf,
    #[serde(default = "default_assemblies_dir")]
    pub assemblies_dir: PathBuf,
    #[serde(
        default = "default_archive_db_path",
        alias = "archive_db_path",
        alias = "repo_db_path"
    )]
    pub archive_db_path: PathBuf,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
    #[serde(default = "default_isolation")]
    pub default_isolation: String,
    #[serde(default)]
    pub ccache: bool,
    #[serde(default)]
    pub memory_limit: Option<u64>,
    #[serde(default)]
    pub cpu_time_limit: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Static per-isolation compiler thread budget. When set, overrides the
    /// dynamic `total_cpus / active_isolations` calculation in the scheduler.
    /// When unset, the scheduler divides CPUs evenly across active isolations.
    #[serde(default)]
    pub nproc_per_isolation: Option<u32>,
    /// Hard cap on the number of CPU cores wright will use in total.
    /// Limits both the parallel isolation count and the dynamic NPROC budget.
    /// Unset = use all available CPUs.
    #[serde(default)]
    pub max_cpus: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkConfig {
    #[serde(default = "default_timeout")]
    pub download_timeout: u64,
    #[serde(default = "default_retry")]
    pub retry_count: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AssembliesConfig {
    #[serde(default)]
    pub assemblies: std::collections::HashMap<String, Assembly>,
}

#[derive(Debug, Deserialize, Clone)]
struct AssemblyFile {
    #[serde(default)]
    assembly: Vec<Assembly>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Assembly {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub plans: Vec<String>,
    #[serde(default)]
    pub includes: Vec<String>,
}

fn default_general() -> GeneralConfig {
    let uid = unsafe { libc::getuid() };
    let use_xdg = uid != 0;

    GeneralConfig {
        arch: default_arch(),
        plans_dir: default_plans_dir(),
        extra_plans_dirs: Vec::new(),
        parts_dir: default_parts_dir(),
        source_dir: if use_xdg {
            get_xdg_cache().unwrap_or_else(default_source_dir)
        } else {
            default_source_dir()
        },
        // by default so `wright resolve ... | sudo wright build ...` consult the
        // same installation state. Per-user overrides can still point db_path
        // elsewhere explicitly.
        installed_db_path: default_installed_db_path(),
        logs_dir: if use_xdg {
            get_xdg_state().unwrap_or_else(default_logs_dir)
        } else {
            default_logs_dir()
        },
        executors_dir: default_executors_dir(),
        assemblies_dir: default_assemblies_dir(),
        archive_db_path: default_archive_db_path(),
    }
}

fn get_xdg_cache() -> Option<PathBuf> {
    std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".cache"))
                .ok()
        })
        .map(|p| p.join("wright/sources"))
}

fn get_xdg_state() -> Option<PathBuf> {
    std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".local/state"))
                .ok()
        })
        .map(|p| p.join("wright"))
}

fn get_xdg_config() -> Option<PathBuf> {
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        return None;
    }

    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .ok()
        })
        .map(|p| p.join("wright/wright.toml"))
}

fn default_arch() -> String {
    "x86_64".to_string()
}
fn default_plans_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/plans")
}
fn default_parts_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/parts")
}
fn default_source_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/sources")
}
fn default_installed_db_path() -> PathBuf {
    PathBuf::from("/var/lib/wright/state/installed.db")
}
fn default_logs_dir() -> PathBuf {
    PathBuf::from("/var/log/wright")
}
fn default_executors_dir() -> PathBuf {
    PathBuf::from("/etc/wright/executors")
}
fn default_assemblies_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/assemblies")
}
fn default_archive_db_path() -> PathBuf {
    PathBuf::from("/var/lib/wright/state/archives.db")
}
fn default_build_dir() -> PathBuf {
    PathBuf::from("/var/tmp/wright/workshop")
}
fn default_isolation() -> String {
    "strict".to_string()
}
fn default_timeout() -> u64 {
    300
}
fn default_retry() -> u32 {
    3
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            general: default_general(),
            build: BuildConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            build_dir: default_build_dir(),
            default_isolation: default_isolation(),
            ccache: false,
            memory_limit: None,
            cpu_time_limit: None,
            timeout: None,
            nproc_per_isolation: None,
            max_cpus: None,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            download_timeout: default_timeout(),
            retry_count: default_retry(),
        }
    }
}

impl AssembliesConfig {
    pub fn load_all(dir: &Path) -> Result<Self> {
        let mut config = AssembliesConfig {
            assemblies: std::collections::HashMap::new(),
        };
        if !dir.exists() {
            return Ok(config);
        }

        for entry in std::fs::read_dir(dir).map_err(WrightError::IoError)? {
            let entry = entry.map_err(WrightError::IoError)?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                let content = std::fs::read_to_string(&path).map_err(|e| {
                    WrightError::ConfigError(format!("failed to read {}: {}", path.display(), e))
                })?;
                let file: AssemblyFile = toml::from_str(&content)?;
                for assembly in file.assembly {
                    config.assemblies.insert(assembly.name.clone(), assembly);
                }
            }
        }
        Ok(config)
    }
}

impl GlobalConfig {
    /// Load configuration with layered merging.
    ///
    /// When an explicit `path` is supplied (via `--config`), that single file
    /// is loaded as-is with no layering.
    ///
    /// Otherwise configs are merged in ascending priority order so that
    /// higher-priority files only need to specify the keys they want to
    /// override — everything else is inherited from the layer below:
    ///
    ///   1. `/etc/wright/wright.toml`          (system-wide, lowest priority)
    ///   2. `$XDG_CONFIG_HOME/wright/wright.toml` (per-user, non-root only)
    ///   3. `./wright.toml`                    (project-local, highest priority)
    ///   4. WRIGHT_* env vars
    pub fn load(path: Option<&Path>) -> Result<Self> {
        use figment::providers::Serialized;
        let mut figment = Figment::from(Serialized::defaults(GlobalConfig::default()));

        if let Some(p) = path {
            figment = figment.merge(Toml::file(p));
        } else {
            figment = figment.merge(Toml::file("/etc/wright/wright.toml"));
            
            if let Some(xdg) = get_xdg_config() {
                figment = figment.merge(Toml::file(xdg));
            }
            
            figment = figment.merge(Toml::file("./wright.toml"));
        }
        
        // Allow env var overrides, e.g., WRIGHT_WORKDIR
        figment = figment.merge(Env::prefixed("WRIGHT_").split("_"));

        figment.extract().map_err(|e| {
            WrightError::ConfigError(format!("Failed to load config: {}", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{default_archive_db_path, default_installed_db_path};

    #[test]
    fn new_default_db_path_names_are_installed_and_archives() {
        assert_eq!(
            default_installed_db_path(),
            PathBuf::from("/var/lib/wright/state/installed.db")
        );
        assert_eq!(
            default_archive_db_path(),
            PathBuf::from("/var/lib/wright/state/archives.db")
        );
    }
}
