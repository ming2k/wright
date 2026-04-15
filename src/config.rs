use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};

#[derive(Debug, Deserialize, Clone)]
pub struct GlobalConfig {
    #[serde(default = "default_general")]
    pub general: GeneralConfig,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

#[derive(Debug, Deserialize, Clone)]
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
    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
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

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    #[serde(default = "default_build_dir")]
    pub build_dir: PathBuf,
    #[serde(default = "default_dockyard")]
    pub default_dockyard: String,
    #[serde(default)]
    pub ccache: bool,
    #[serde(default)]
    pub memory_limit: Option<u64>,
    #[serde(default)]
    pub cpu_time_limit: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Static per-dockyard compiler thread budget. When set, overrides the
    /// dynamic `total_cpus / active_dockyards` calculation in the scheduler.
    /// When unset, the scheduler divides CPUs evenly across active dockyards.
    #[serde(default)]
    pub nproc_per_dockyard: Option<u32>,
    /// Hard cap on the number of CPU cores wright will use in total.
    /// Limits both the parallel dockyard count and the dynamic NPROC budget.
    /// Unset = use all available CPUs.
    #[serde(default)]
    pub max_cpus: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
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
        // Use the system-installed part database for both root and non-root
        // by default so `wright resolve ... | sudo wright build ...` consult the
        // same installation state. Per-user overrides can still point db_path
        // elsewhere explicitly.
        installed_db_path: default_installed_db_path(),
        log_dir: if use_xdg {
            get_xdg_state().unwrap_or_else(default_log_dir)
        } else {
            default_log_dir()
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
fn default_log_dir() -> PathBuf {
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
    PathBuf::from("/var/tmp/wright-build")
}
fn default_dockyard() -> String {
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
            default_dockyard: default_dockyard(),
            ccache: false,
            memory_limit: None,
            cpu_time_limit: None,
            timeout: None,
            nproc_per_dockyard: None,
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

/// Recursively merge two TOML values. For tables, overlay keys win;
/// missing keys are inherited from base. All other types (scalars, arrays)
/// are replaced wholesale by the overlay value.
fn merge_toml(base: toml::Value, overlay: toml::Value) -> toml::Value {
    use toml::Value;
    match (base, overlay) {
        (Value::Table(mut base_map), Value::Table(overlay_map)) => {
            for (k, v) in overlay_map {
                let merged = if let Some(base_v) = base_map.remove(&k) {
                    merge_toml(base_v, v)
                } else {
                    v
                };
                base_map.insert(k, merged);
            }
            Value::Table(base_map)
        }
        // Scalars and arrays: overlay wins unconditionally
        (_, overlay) => overlay,
    }
}

fn load_toml_file(path: &Path) -> Result<toml::Value> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        WrightError::ConfigError(format!("failed to read {}: {}", path.display(), e))
    })?;
    Ok(toml::from_str(&content)?)
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
    ///
    /// Any layer that does not exist is silently skipped. If no file is found
    /// at any location, built-in defaults are used.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        // Explicit path: single-file load, no layering.
        if let Some(p) = path {
            let config_path = PathBuf::from(p);
            if !config_path.exists() {
                return Ok(Self::default());
            }
            return Ok(toml::from_str(
                &std::fs::read_to_string(&config_path).map_err(|e| {
                    WrightError::ConfigError(format!(
                        "failed to read {}: {}",
                        config_path.display(),
                        e
                    ))
                })?,
            )?);
        }

        // Layered load: accumulate from lowest to highest priority.
        let mut layers: Vec<PathBuf> = vec![PathBuf::from("/etc/wright/wright.toml")];
        if let Some(xdg) = get_xdg_config() {
            layers.push(xdg);
        }
        layers.push(PathBuf::from("./wright.toml"));

        let mut merged: Option<toml::Value> = None;
        for layer_path in &layers {
            if layer_path.exists() {
                let val = load_toml_file(layer_path)?;
                merged = Some(match merged {
                    Some(base) => merge_toml(base, val),
                    None => val,
                });
            }
        }

        match merged {
            None => Ok(Self::default()),
            Some(val) => {
                use serde::Deserialize;
                Ok(GlobalConfig::deserialize(val)?)
            }
        }
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
