use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod error;
pub use error::{ConfigError, Result};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GlobalConfig {
    #[serde(default = "default_general")]
    pub general: GeneralConfig,
    #[serde(default)]
    pub build: ForgeConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeneralConfig {
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default = "default_plans_dir")]
    pub plans_dir: PathBuf,
    /// Directory containing folio manifest files (`<name>.toml`).
    /// Defaults to the `folios` sibling of `plans_dir`.
    #[serde(default = "default_folios_dir")]
    pub folios_dir: PathBuf,
    /// Additional plan search directories consulted after `plans_dir`.
    /// Relative paths are resolved against the working directory at runtime.
    #[serde(default)]
    pub extra_plans_dirs: Vec<PathBuf>,
    #[serde(default = "default_parts_dir", alias = "components_dir")]
    pub parts_dir: PathBuf,
    #[serde(default = "default_store_dir")]
    pub store_dir: PathBuf,
    #[serde(default = "default_source_dir")]
    pub source_dir: PathBuf,
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,
    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,
    /// Machine-local audit ledger: per-plan build-cost records
    /// (`<plan>/builds.jsonl`) and plan-source snapshots
    /// (`<plan>/snapshots/`). Deliberately NOT under `plans_dir`: the ledger
    /// records facts about this machine, while plans are portable source.
    /// Defaults to the system location even for non-root users so builds and
    /// deploys across privilege boundaries share one ledger (same stance as
    /// `db_path`); writes are best-effort and never fail a build or deploy.
    #[serde(default = "default_ledger_dir")]
    pub ledger_dir: PathBuf,
    #[serde(default = "default_executors_dir")]
    pub executors_dir: PathBuf,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    #[serde(default = "default_forge_dir")]
    pub forge_dir: PathBuf,
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
    /// Plans exempted from cascade-rebuild expansion when using `--rdeps`.
    /// These are treated as stable base toolchain parts whose reverse-dependents
    /// should not be rebuilt automatically. Override in `wright.toml` if your
    /// distribution uses different package names.
    #[serde(default = "default_stable_toolchain")]
    pub stable_toolchain: Vec<String>,
    /// Inhibit reverse-dependency rebuilds when the updated package's physical
    /// ABI probe proves backward compatibility. Defaults to true.
    #[serde(default = "default_true")]
    pub inhibit_abi_rebuild: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct NetworkConfig {
    #[serde(default = "default_timeout")]
    pub download_timeout: u64,
    #[serde(default = "default_retry")]
    pub retry_count: u32,
    /// Maximum number of concurrent source downloads across the whole
    /// process. Prevents thundering-herd on mirror servers when a batch
    /// triggers many parallel fetches.
    #[serde(default = "default_max_concurrent_downloads")]
    pub max_concurrent_downloads: usize,
}

fn default_general() -> GeneralConfig {
    let uid = unsafe { libc::getuid() };
    let use_xdg = uid != 0;

    GeneralConfig {
        arch: default_arch(),
        plans_dir: default_plans_dir(),
        folios_dir: default_folios_dir(),
        extra_plans_dirs: Vec::new(),
        parts_dir: default_parts_dir(),
        store_dir: default_store_dir(),
        source_dir: if use_xdg {
            get_xdg_cache().unwrap_or_else(default_source_dir)
        } else {
            default_source_dir()
        },
        // by default so `wright resolve ... | sudo wright build ...` consult the
        // same installation state. Per-user overrides can still point db_path
        // elsewhere explicitly.
        db_path: default_db_path(),
        logs_dir: if use_xdg {
            get_xdg_state().unwrap_or_else(default_logs_dir)
        } else {
            default_logs_dir()
        },
        ledger_dir: default_ledger_dir(),
        executors_dir: default_executors_dir(),
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
fn default_folios_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/folios")
}
fn default_parts_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/parts")
}
fn default_store_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/store")
}
fn default_source_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/sources")
}
fn default_db_path() -> PathBuf {
    PathBuf::from("/var/lib/wright/wright.db")
}
fn default_logs_dir() -> PathBuf {
    PathBuf::from("/var/log/wright")
}
fn default_ledger_dir() -> PathBuf {
    PathBuf::from("/var/lib/wright/ledger")
}
fn default_executors_dir() -> PathBuf {
    PathBuf::from("/etc/wright/executors")
}
fn default_forge_dir() -> PathBuf {
    PathBuf::from("/var/tmp/wright/workshop")
}
fn default_true() -> bool {
    true
}
fn default_isolation() -> String {
    "strict".to_string()
}
fn default_stable_toolchain() -> Vec<String> {
    [
        "gcc", "glibc", "binutils", "make", "bison", "flex", "perl", "python", "texinfo", "m4",
        "sed", "gawk",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
fn default_timeout() -> u64 {
    300
}
fn default_retry() -> u32 {
    3
}
fn default_max_concurrent_downloads() -> usize {
    8
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            general: default_general(),
            build: ForgeConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            forge_dir: default_forge_dir(),
            default_isolation: default_isolation(),
            ccache: false,
            memory_limit: None,
            cpu_time_limit: None,
            timeout: None,
            nproc_per_isolation: None,
            max_cpus: None,
            stable_toolchain: default_stable_toolchain(),
            inhibit_abi_rebuild: true,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            download_timeout: default_timeout(),
            retry_count: default_retry(),
            max_concurrent_downloads: default_max_concurrent_downloads(),
        }
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

        // Allow env var overrides, e.g., WRIGHT_BUILD_FORGE_DIR
        figment = figment.merge(Env::prefixed("WRIGHT_").split("_"));

        let config: Self = figment
            .extract()
            .map_err(|e| ConfigError::Figment(Box::new(e)))?;
        config
            .build
            .default_isolation
            .parse::<wright_model::isolation::IsolationLevel>()
            .map_err(|error| {
                ConfigError::Message(format!(
                    "invalid build.default_isolation '{}': {error}",
                    config.build.default_isolation
                ))
            })?;
        config.validate_storage_layout(&config.general.db_path)?;
        Ok(config)
    }

    /// Reject storage layouts where a cache purge could delete something that
    /// is not a cache.
    ///
    /// `source_dir` and `store_dir` are the only directories a cold build may
    /// purge. If either is equal to, contains, or is contained by the part
    /// inventory, the database, the ledger, logs, or the forge workspace, a
    /// purge would destroy durable state. `plans_dir`, `folios_dir`, and
    /// `executors_dir` are protected in one direction: a cache root must not
    /// contain them, but a cache nested inside them is harmless (the purge only
    /// walks the cache itself).
    ///
    /// Paths that exist are canonicalised first, so a symlink cannot disguise
    /// an overlap. Paths that do not exist yet are compared lexically.
    pub fn validate_storage_layout(&self, db_path: &Path) -> Result<()> {
        #[derive(Clone, Copy)]
        enum Overlap {
            /// Reject both containment directions.
            Symmetric,
            /// Reject only "cache contains protected".
            ContainedOnly,
        }

        let cache_roots = [
            ("general.source_dir", self.general.source_dir.clone()),
            ("general.store_dir", self.general.store_dir.clone()),
        ];

        let mut protected: Vec<(String, PathBuf, Overlap)> = vec![
            (
                "general.parts_dir".to_string(),
                self.general.parts_dir.clone(),
                Overlap::Symmetric,
            ),
            (
                "db_path".to_string(),
                db_path.to_path_buf(),
                Overlap::Symmetric,
            ),
            (
                "general.ledger_dir".to_string(),
                self.general.ledger_dir.clone(),
                Overlap::Symmetric,
            ),
            (
                "general.logs_dir".to_string(),
                self.general.logs_dir.clone(),
                Overlap::Symmetric,
            ),
            (
                "build.forge_dir".to_string(),
                self.build.forge_dir.clone(),
                Overlap::Symmetric,
            ),
            (
                "general.plans_dir".to_string(),
                self.general.plans_dir.clone(),
                Overlap::ContainedOnly,
            ),
            (
                "general.folios_dir".to_string(),
                self.general.folios_dir.clone(),
                Overlap::ContainedOnly,
            ),
            (
                "general.executors_dir".to_string(),
                self.general.executors_dir.clone(),
                Overlap::ContainedOnly,
            ),
        ];
        for (idx, dir) in self.general.extra_plans_dirs.iter().enumerate() {
            protected.push((
                format!("general.extra_plans_dirs[{idx}]"),
                dir.clone(),
                Overlap::ContainedOnly,
            ));
        }

        // The two cache roots must not overlap each other.
        let (source_key, source) = &cache_roots[0];
        let (store_key, store) = &cache_roots[1];
        if paths_overlap(source, store) {
            return Err(storage_conflict(source_key, store_key, source, store));
        }

        for (cache_key, cache_root) in &cache_roots {
            let cache = normalize_path(cache_root);
            for (protected_key, protected_path, mode) in &protected {
                let protected = normalize_path(protected_path);
                let overlaps = match mode {
                    Overlap::Symmetric => paths_overlap(&cache, &protected),
                    Overlap::ContainedOnly => is_ancestor_or_equal(&cache, &protected),
                };
                if overlaps {
                    return Err(storage_conflict(
                        cache_key,
                        protected_key,
                        &cache,
                        &protected,
                    ));
                }
            }
        }
        Ok(())
    }
}

fn storage_conflict(cache_key: &str, other_key: &str, a: &Path, b: &Path) -> ConfigError {
    ConfigError::Message(format!(
        "storage layout conflict: `{cache_key}` ({}) and `{other_key}` ({}) overlap; \
         a cold cache purge would delete non-cache data. Make these paths disjoint.",
        a.display(),
        b.display()
    ))
}

fn paths_overlap(a: &Path, b: &Path) -> bool {
    let a = normalize_path(a);
    let b = normalize_path(b);
    is_ancestor_or_equal(&a, &b) || is_ancestor_or_equal(&b, &a)
}

fn is_ancestor_or_equal(a: &Path, b: &Path) -> bool {
    b == a || b.starts_with(a)
}

/// Canonicalise existing paths; for paths that do not exist yet, canonicalise
/// the nearest existing ancestor and append the remainder. This makes a
/// symlinked parent impossible to use as a disguise for an overlap.
fn normalize_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    if let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return canonical;
    }

    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = absolute.as_path();
    while let Some(name) = cursor.file_name() {
        suffix.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            break;
        };
        if let Ok(mut canonical) = std::fs::canonicalize(parent) {
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return lexical_normalize(&canonical);
        }
        cursor = parent;
    }
    lexical_normalize(&absolute)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{GlobalConfig, default_db_path};

    #[test]
    fn new_default_db_path() {
        assert_eq!(
            default_db_path(),
            PathBuf::from("/var/lib/wright/wright.db")
        );
    }

    #[test]
    fn rejects_invalid_default_isolation_during_load() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("wright.toml");
        std::fs::write(&path, "[build]\ndefault_isolation = \"container\"\n").unwrap();

        let error = GlobalConfig::load(Some(&path)).unwrap_err().to_string();
        assert!(error.contains("invalid build.default_isolation"));
    }

    #[test]
    fn rejects_a_cache_root_that_contains_the_part_inventory() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("cache");
        config.general.store_dir = root.path().join("store");
        config.general.parts_dir = root.path().join("cache/parts");

        let error = config
            .validate_storage_layout(&root.path().join("wright.db"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("storage layout conflict"));
        assert!(error.contains("general.source_dir"));
        assert!(error.contains("general.parts_dir"));
    }

    #[test]
    fn rejects_equal_cache_roots() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("cache");
        config.general.store_dir = root.path().join("cache");
        config.general.parts_dir = root.path().join("parts");

        let error = config
            .validate_storage_layout(&root.path().join("wright.db"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("general.source_dir"));
        assert!(error.contains("general.store_dir"));
    }

    #[test]
    fn rejects_a_cache_root_that_contains_the_database() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("sources");
        config.general.store_dir = root.path().join("store");
        config.general.parts_dir = root.path().join("parts");

        let error = config
            .validate_storage_layout(&root.path().join("sources/wright.db"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("db_path"));
    }

    #[test]
    fn rejects_a_cache_root_that_contains_the_plan_tree() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.path().join("cache");
        config.general.store_dir = root.path().join("store");
        config.general.parts_dir = root.path().join("parts");
        config.general.plans_dir = root.path().join("cache/plans");

        let error = config
            .validate_storage_layout(&root.path().join("wright.db"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("general.plans_dir"));
    }

    #[test]
    fn allows_sibling_paths_and_a_cache_nested_under_the_plan_tree() {
        let root = tempfile::tempdir().unwrap();
        let mut config = GlobalConfig::default();
        config.general.plans_dir = root.path().join("plans");
        config.general.folios_dir = root.path().join("folios");
        config.general.parts_dir = root.path().join("parts");
        config.general.store_dir = root.path().join("store");
        config.general.source_dir = root.path().join("plans/.wright/sources");
        config.general.logs_dir = root.path().join("logs");
        config.general.ledger_dir = root.path().join("ledger");
        config.general.executors_dir = root.path().join("executors");
        config.build.forge_dir = root.path().join("forge");

        config
            .validate_storage_layout(&root.path().join("wright.db"))
            .expect("sibling storage roots and a cache under plans_dir are safe");
    }

    #[cfg(unix)]
    #[test]
    fn resolves_symlinks_before_comparing_layouts() {
        let root = tempfile::tempdir().unwrap();
        let real_parts = root.path().join("parts");
        std::fs::create_dir_all(&real_parts).unwrap();
        let source_link = root.path().join("sources");
        std::os::unix::fs::symlink(&real_parts, &source_link).unwrap();

        let mut config = GlobalConfig::default();
        config.general.source_dir = source_link;
        config.general.store_dir = root.path().join("store");
        config.general.parts_dir = real_parts;

        let error = config
            .validate_storage_layout(&root.path().join("wright.db"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("storage layout conflict"));
    }

    #[cfg(unix)]
    #[test]
    fn resolves_a_symlinked_ancestor_of_a_nonexistent_cache_root() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut config = GlobalConfig::default();
        // Neither path exists, but the symlinked parent makes them the same
        // tree; the purge must not be allowed to walk it.
        config.general.source_dir = link.join("sources");
        config.general.store_dir = root.path().join("store");
        config.general.parts_dir = real.join("sources/parts");

        let error = config
            .validate_storage_layout(&root.path().join("wright.db"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("storage layout conflict"));
    }

    #[test]
    fn load_rejects_an_overlapping_layout() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("wright.toml");
        let parts = root.path().join("parts");
        std::fs::write(
            &path,
            format!(
                "[general]\nsource_dir = \"{}\"\nparts_dir = \"{}\"\nstore_dir = \"{}\"\ndb_path = \"{}\"\n",
                parts.display(),
                parts.display(),
                root.path().join("store").display(),
                root.path().join("wright.db").display(),
            ),
        )
        .unwrap();

        let error = GlobalConfig::load(Some(&path)).unwrap_err().to_string();
        assert!(error.contains("storage layout conflict"));
    }
}
