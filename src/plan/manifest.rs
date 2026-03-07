use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{WrightError, Result};

// ---------------------------------------------------------------------------
// Fabricate output types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone, Default)]
pub struct FabricateHooks {
    #[serde(default)]
    pub pre_install: Option<String>,
    #[serde(default)]
    pub post_install: Option<String>,
    #[serde(default)]
    pub post_upgrade: Option<String>,
    #[serde(default)]
    pub pre_remove: Option<String>,
    #[serde(default)]
    pub post_remove: Option<String>,
}

/// Main output mode: `[lifecycle.fabricate]`
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct FabricateOutput {
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub backup: Option<Vec<String>>,
}

/// Additional output mode: `[lifecycle.fabricate.<name>]`
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SubFabricateOutput {
    #[serde(default)]
    pub description: Option<String>,
    pub version: Option<String>,
    pub release: Option<u32>,
    pub arch: Option<String>,
    pub license: Option<String>,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default)]
    pub script: String,
    #[serde(default = "default_executor")]
    pub executor: String,
    #[serde(default = "default_dockyard_level")]
    pub dockyard: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub backup: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum FabricateConfig {
    Single(FabricateOutput),
    Multi(HashMap<String, SubFabricateOutput>),
}

// ---------------------------------------------------------------------------
// Archive metadata helper types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct InstallScripts {
    #[serde(default)]
    pub pre_install: Option<String>,
    #[serde(default)]
    pub post_install: Option<String>,
    #[serde(default)]
    pub post_upgrade: Option<String>,
    #[serde(default)]
    pub pre_remove: Option<String>,
    #[serde(default)]
    pub post_remove: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BackupConfig {
    #[serde(default)]
    pub files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Main manifest
// ---------------------------------------------------------------------------

/// Package relations (replaces, conflicts, provides).
/// Moved from [dependencies] to [relations] in v1.3.1.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct Relations {
    #[serde(default)]
    pub replaces: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
}

/// A single source entry in the `[[sources]]` array-of-tables format.
#[derive(Debug, Deserialize, Clone)]
pub struct Source {
    pub uri: String,
    #[serde(default = "default_skip")]
    pub sha256: String,
}

fn default_skip() -> String {
    "SKIP".to_string()
}

#[derive(Debug, Clone)]
pub struct PlanManifest {
    pub plan: PackageMetadata,
    pub dependencies: Dependencies,
    pub relations: Relations,
    pub sources: Sources,
    pub options: BuildOptions,
    pub lifecycle: HashMap<String, LifecycleStage>,
    pub lifecycle_order: Option<LifecycleOrder>,
    pub mvp: Option<PhaseConfig>,
    /// Fabricate output configuration.
    pub fabricate: Option<FabricateConfig>,
    /// Derived archive metadata populated from `fabricate`.
    pub install_scripts: Option<InstallScripts>,
    pub backup: Option<BackupConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    pub release: u32,
    #[serde(default)]
    pub epoch: u32,
    pub description: String,
    pub license: String,
    pub arch: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub maintainer: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Dependencies {
    #[serde(default)]
    pub runtime: Vec<String>,
    #[serde(default)]
    pub build: Vec<String>,
    #[serde(default)]
    pub link: Vec<String>,
    #[serde(default)]
    pub optional: Vec<OptionalDependency>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OptionalDependency {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub entries: Vec<Source>,
}

impl Sources {
    pub fn uris(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.uri.as_str())
    }

    pub fn sha256s(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.sha256.as_str())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BuildOptions {
    #[serde(default = "default_true")]
    pub strip: bool,
    #[serde(default, rename = "static")]
    pub static_: bool,
    #[serde(default)]
    pub debug: bool,
    #[serde(default = "default_true")]
    pub ccache: bool,
    /// Package-wide environment variables injected into every lifecycle stage.
    /// Per-stage `[lifecycle.<stage>.env]` takes precedence over these.
    /// Use this to set tool-specific parallelism (e.g. MAKEFLAGS, GOFLAGS)
    /// or any other build knobs the script needs.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub memory_limit: Option<u64>,
    #[serde(default)]
    pub cpu_time_limit: Option<u64>,
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Skip FHS validation after the final output stage.
    /// Set to `true` only for packages with a deliberate reason to install
    /// outside the standard FHS paths (e.g. kernel modules, legacy compat layers).
    #[serde(default)]
    pub skip_fhs_check: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            strip: true,
            static_: false,
            debug: false,
            ccache: true,
            env: std::collections::HashMap::new(),
            memory_limit: None,
            cpu_time_limit: None,
            timeout: None,
            skip_fhs_check: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone)]
pub struct LifecycleStage {
    #[serde(default = "default_executor")]
    pub executor: String,
    #[serde(default = "default_dockyard_level")]
    pub dockyard: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub script: String,
}

fn default_executor() -> String {
    "shell".to_string()
}

fn default_dockyard_level() -> String {
    "strict".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct LifecycleOrder {
    pub stages: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PhaseConfig {
    /// Phase-specific dependency overrides. Any field omitted falls back
    /// to the top-level [dependencies].
    #[serde(default)]
    pub dependencies: Option<PhaseDependencies>,
    #[serde(default)]
    pub lifecycle: HashMap<String, LifecycleStage>,
    #[serde(default)]
    pub lifecycle_order: Option<LifecycleOrder>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct PhaseDependencies {
    #[serde(default)]
    pub runtime: Option<Vec<String>>,
    #[serde(default)]
    pub build: Option<Vec<String>>,
    #[serde(default)]
    pub link: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Raw manifest for custom deserialization
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    plan: PackageMetadata,
    #[serde(default)]
    dependencies: Dependencies,
    #[serde(default)]
    relations: Option<Relations>,
    #[serde(default)]
    sources: Option<toml::Value>,
    #[serde(default)]
    options: BuildOptions,
    #[serde(default)]
    lifecycle: Option<HashMap<String, toml::Value>>,
    #[serde(default)]
    lifecycle_order: Option<LifecycleOrder>,
    #[serde(default)]
    mvp: Option<PhaseConfig>,
}

// ---------------------------------------------------------------------------
// SubFabricateOutput -> PlanManifest conversion
// ---------------------------------------------------------------------------

impl SubFabricateOutput {
    /// Produce a full PlanManifest for archive creation, inheriting from the parent.
    pub fn to_manifest(&self, name: &str, parent: &PlanManifest) -> PlanManifest {
        let description = self.description.clone()
            .unwrap_or_else(|| parent.plan.description.clone());

        // Convert hooks to legacy InstallScripts for archive creation
        let install_scripts = self.hooks.as_ref().map(|h| InstallScripts {
            pre_install: h.pre_install.clone(),
            post_install: h.post_install.clone(),
            post_upgrade: h.post_upgrade.clone(),
            pre_remove: h.pre_remove.clone(),
            post_remove: h.post_remove.clone(),
        });

        let backup = self.backup.as_ref().map(|files| BackupConfig {
            files: files.clone(),
        });

        PlanManifest {
            plan: PackageMetadata {
                name: name.to_string(),
                version: self.version.clone().unwrap_or_else(|| parent.plan.version.clone()),
                release: self.release.unwrap_or(parent.plan.release),
                epoch: parent.plan.epoch,
                description,
                license: self.license.clone().unwrap_or_else(|| parent.plan.license.clone()),
                arch: self.arch.clone().unwrap_or_else(|| parent.plan.arch.clone()),
                url: parent.plan.url.clone(),
                maintainer: parent.plan.maintainer.clone(),
            },
            dependencies: self.dependencies.clone(),
            relations: Relations::default(),
            sources: Sources::default(),
            options: BuildOptions::default(),
            lifecycle: HashMap::new(),
            lifecycle_order: None,
            mvp: None,
            fabricate: None,
            install_scripts,
            backup,
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn fabricate_install_scripts(output: &FabricateOutput) -> Option<InstallScripts> {
    output.hooks.as_ref().map(|h| InstallScripts {
        pre_install: h.pre_install.clone(),
        post_install: h.post_install.clone(),
        post_upgrade: h.post_upgrade.clone(),
        pre_remove: h.pre_remove.clone(),
        post_remove: h.post_remove.clone(),
    })
}

fn fabricate_backup(output: &FabricateOutput) -> Option<BackupConfig> {
    output.backup.as_ref().map(|files| BackupConfig {
        files: files.clone(),
    })
}

fn empty_sub_fabricate_output(hooks: Option<FabricateHooks>, backup: Option<Vec<String>>) -> SubFabricateOutput {
    SubFabricateOutput {
        description: None,
        version: None,
        release: None,
        arch: None,
        license: None,
        dependencies: Dependencies::default(),
        script: String::new(),
        executor: default_executor(),
        dockyard: default_dockyard_level(),
        env: HashMap::new(),
        hooks,
        backup,
    }
}

fn parse_fabricate_section(
    plan_name: &str,
    fabricate_val: toml::Value,
) -> Result<(Option<LifecycleStage>, Option<FabricateConfig>, Option<InstallScripts>, Option<BackupConfig>)> {
    let mut table = match fabricate_val {
        toml::Value::Table(table) => table,
        _ => {
            return Err(WrightError::ParseError(
                "[lifecycle.fabricate] must be a table".to_string()
            ));
        }
    };

    let mut stage_table = toml::value::Table::new();
    for key in ["executor", "dockyard", "env", "script"] {
        if let Some(value) = table.remove(key) {
            stage_table.insert(key.to_string(), value);
        }
    }

    let stage = if stage_table.is_empty() {
        None
    } else {
        Some(
            toml::Value::Table(stage_table)
                .try_into()
                .map_err(|e: toml::de::Error| {
                    WrightError::ParseError(format!(
                        "failed to parse [lifecycle.fabricate] stage fields: {}", e
                    ))
                })?,
        )
    };

    let hooks = match table.remove("hooks") {
        Some(value) => Some(value.try_into().map_err(|e: toml::de::Error| {
            WrightError::ParseError(format!(
                "failed to parse [lifecycle.fabricate].hooks: {}", e
            ))
        })?),
        None => None,
    };

    let backup = match table.remove("backup") {
        Some(value) => Some(value.try_into().map_err(|e: toml::de::Error| {
            WrightError::ParseError(format!(
                "failed to parse [lifecycle.fabricate].backup: {}", e
            ))
        })?),
        None => None,
    };

    let main_output = FabricateOutput { hooks, backup };
    let install_scripts = fabricate_install_scripts(&main_output);
    let backup_cfg = fabricate_backup(&main_output);

    if table.is_empty() {
        let fabricate = if main_output.hooks.is_some() || main_output.backup.is_some() {
            Some(FabricateConfig::Single(main_output))
        } else {
            None
        };
        return Ok((stage, fabricate, install_scripts, backup_cfg));
    }

    let table_value = toml::Value::Table(table);
    let mut outputs: HashMap<String, SubFabricateOutput> =
        table_value.try_into().map_err(|e: toml::de::Error| {
            WrightError::ParseError(format!(
                "failed to parse [lifecycle.fabricate.<name>]: {}", e
            ))
        })?;

    if outputs.contains_key(plan_name) {
        return Err(WrightError::ParseError(format!(
            "main output '{}' must use [lifecycle.fabricate], not [lifecycle.fabricate.{}]",
            plan_name, plan_name
        )));
    }

    outputs.insert(
        plan_name.to_string(),
        empty_sub_fabricate_output(main_output.hooks.clone(), main_output.backup.clone()),
    );

    Ok((
        stage,
        Some(FabricateConfig::Multi(outputs)),
        install_scripts,
        backup_cfg,
    ))
}

impl PlanManifest {
    pub fn parse(content: &str) -> Result<Self> {
        let raw: RawManifest = toml::from_str(content)?;

        // --- Parse sources ---
        let sources = match raw.sources {
            Some(toml::Value::Array(arr)) => {
                let mut entries = Vec::new();
                for (i, val) in arr.into_iter().enumerate() {
                    let entry: Source = val.try_into().map_err(|e: toml::de::Error| {
                        WrightError::ParseError(format!(
                            "failed to parse [[sources]] entry {}: {}", i, e
                        ))
                    })?;
                    entries.push(entry);
                }
                Sources { entries }
            }
            Some(toml::Value::Table(_)) => {
                return Err(WrightError::ParseError(
                    "sources must use [[sources]] array-of-tables".to_string()
                ));
            }
            None => Sources::default(),
            _ => {
                return Err(WrightError::ParseError(
                    "sources must be an array-of-tables ([[sources]])".to_string()
                ));
            }
        };

        // --- Parse relations ---
        let relations = raw.relations.unwrap_or_default();

        // --- Extract lifecycle stages and parse [lifecycle.fabricate] ---
        let mut lifecycle_stages: HashMap<String, LifecycleStage> = HashMap::new();
        let mut lifecycle_fabricate_value: Option<toml::Value> = None;
        let mut fabricate: Option<FabricateConfig> = None;
        let mut install_scripts: Option<InstallScripts> = None;
        let mut backup: Option<BackupConfig> = None;

        if let Some(raw_lifecycle) = raw.lifecycle {
            for (key, value) in raw_lifecycle {
                if key == "fabricate" {
                    lifecycle_fabricate_value = Some(value);
                } else if key == "part" {
                    return Err(WrightError::ParseError(
                        "[lifecycle.part] is no longer supported; use [lifecycle.fabricate]".to_string()
                    ));
                } else if key == "package" {
                    return Err(WrightError::ParseError(
                        "[lifecycle.package] is no longer supported; use [lifecycle.fabricate]".to_string()
                    ));
                } else {
                    let stage: LifecycleStage = value.try_into().map_err(|e: toml::de::Error| {
                        WrightError::ParseError(format!(
                            "failed to parse lifecycle stage '{}': {}", key, e
                        ))
                    })?;
                    lifecycle_stages.insert(key, stage);
                }
            }
        }

        if let Some(fabricate_val) = lifecycle_fabricate_value {
            let (stage, fabricate_cfg, scripts, backup_cfg) =
                parse_fabricate_section(&raw.plan.name, fabricate_val)?;
            if let Some(stage) = stage {
                lifecycle_stages.insert("fabricate".to_string(), stage);
            }
            fabricate = fabricate_cfg;
            install_scripts = scripts;
            backup = backup_cfg;
        }

        let manifest = PlanManifest {
            plan: raw.plan,
            dependencies: raw.dependencies,
            relations,
            sources,
            options: raw.options,
            lifecycle: lifecycle_stages,
            lifecycle_order: raw.lifecycle_order,
            mvp: raw.mvp,
            fabricate,
            install_scripts,
            backup,
        };

        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            WrightError::ParseError(format!("failed to read {}: {}", path.display(), e))
        })?;
        Self::parse(&content)
    }

    pub fn validate(&self) -> Result<()> {
        let name_re = regex::Regex::new(r"^[a-z0-9][a-z0-9_+.-]*$").unwrap();
        if !name_re.is_match(&self.plan.name) {
            return Err(WrightError::ValidationError(format!(
                "invalid package name '{}': must match [a-z0-9][a-z0-9_+.-]*",
                self.plan.name
            )));
        }
        if self.plan.name.len() > 64 {
            return Err(WrightError::ValidationError(
                "package name must be at most 64 characters".to_string(),
            ));
        }

        // Validate version parses
        crate::part::version::Version::parse(&self.plan.version)?;

        if self.plan.release == 0 {
            return Err(WrightError::ValidationError(
                "release must be >= 1".to_string(),
            ));
        }

        if self.plan.description.is_empty() {
            return Err(WrightError::ValidationError(
                "description must not be empty".to_string(),
            ));
        }

        if self.plan.license.is_empty() {
            return Err(WrightError::ValidationError(
                "license must not be empty".to_string(),
            ));
        }

        if self.plan.arch.is_empty() {
            return Err(WrightError::ValidationError(
                "arch must not be empty".to_string(),
            ));
        }

        // Validate lifecycle stage names
        let stages: Vec<&str> = if let Some(ref order) = self.lifecycle_order {
            order.stages.iter().map(|s| s.as_str()).collect()
        } else {
            crate::builder::lifecycle::DEFAULT_STAGES.to_vec()
        };
        let mut valid_names = std::collections::HashSet::new();
        for stage in &stages {
            valid_names.insert(stage.to_string());
            valid_names.insert(format!("pre_{}", stage));
            valid_names.insert(format!("post_{}", stage));
        }
        for key in self.lifecycle.keys() {
            if !valid_names.contains(key) {
                return Err(WrightError::ValidationError(format!(
                    "unknown lifecycle stage '{}'. Valid stages: {}",
                    key,
                    stages.iter()
                        .filter(|s| !["fetch", "verify", "extract"].contains(s))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }

        // Each source entry is self-contained (uri + sha256), no positional check needed

        // Validate fabricate config
        if let Some(ref pkg) = self.fabricate {
            match pkg {
                FabricateConfig::Multi(ref packages) => {
                    for (sub_name, sub_part) in packages {
                        if !name_re.is_match(sub_name) {
                            return Err(WrightError::ValidationError(format!(
                                "invalid sub-package name '{}': must match [a-z0-9][a-z0-9_+.-]*",
                                sub_name
                            )));
                        }
                        // Non-main sub-packages must have description
                        if sub_name != &self.plan.name && sub_part.description.is_none() {
                            return Err(WrightError::ValidationError(format!(
                                "sub-package '{}': description is required for non-main packages",
                                sub_name
                            )));
                        }
                        if let Some(ref ver) = sub_part.version {
                            crate::part::version::Version::parse(ver)?;
                        }
                        if let Some(ref rel) = sub_part.release {
                            if *rel == 0 {
                                return Err(WrightError::ValidationError(format!(
                                    "sub-package '{}': release must be >= 1",
                                    sub_name
                                )));
                            }
                        }
                    }
                }
                FabricateConfig::Single(_) => {
                    // No special validation needed for single mode
                }
            }
        }

        Ok(())
    }

    /// Get the archive filename for this package.
    /// Includes epoch only when > 0: `name-epoch:version-release-arch.wright.tar.zst`
    pub fn archive_filename(&self) -> String {
        if self.plan.epoch > 0 {
            format!(
                "{}-{}:{}-{}-{}.wright.tar.zst",
                self.plan.name, self.plan.epoch, self.plan.version, self.plan.release, self.plan.arch
            )
        } else {
            format!(
                "{}-{}-{}-{}.wright.tar.zst",
                self.plan.name, self.plan.version, self.plan.release, self.plan.arch
            )
        }
    }

    /// Iterate over sub-packages (multi-package mode).
    /// Returns an empty iterator for Single or None.
    pub fn sub_packages(&self) -> impl Iterator<Item = (&String, &SubFabricateOutput)> {
        match self.fabricate {
            Some(FabricateConfig::Multi(ref pkgs)) => {
                Box::new(pkgs.iter()) as Box<dyn Iterator<Item = _>>
            }
            _ => Box::new(std::iter::empty()),
        }
    }

    /// Get sub-packages that are not the main package (need their own script/PART_DIR).
    pub fn extra_sub_packages(&self) -> impl Iterator<Item = (&String, &SubFabricateOutput)> {
        let main_name = self.plan.name.clone();
        self.sub_packages().filter(move |(name, _)| *name != &main_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hello_fixture() {
        let toml_str = r#"
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test package"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = []
build = ["gcc"]

[lifecycle.prepare]
executor = "shell"
dockyard = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.compile]
executor = "shell"
dockyard = "none"
script = """
gcc -o hello hello.c
"""

[lifecycle.staging]
executor = "shell"
dockyard = "none"
script = """
install -Dm755 hello ${PART_DIR}/usr/bin/hello
"""
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.name, "hello");
        assert_eq!(manifest.plan.version, "1.0.0");
        assert_eq!(manifest.plan.release, 1);
        assert_eq!(manifest.plan.arch, "x86_64");
        assert_eq!(manifest.plan.epoch, 0);
        assert_eq!(manifest.dependencies.build, vec!["gcc"]);
        assert!(manifest.lifecycle.contains_key("prepare"));
        assert!(manifest.lifecycle.contains_key("compile"));
        assert!(manifest.lifecycle.contains_key("staging"));
    }

    #[test]
    fn test_parse_full_featured() {
        let toml_str = r#"
[plan]
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Test <test@test.com>"

[dependencies]
runtime = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]
build = ["perl", "gcc", "make"]
optional = [
    { name = "geoip", description = "GeoIP module support" },
]

[relations]
conflicts = ["apache"]
provides = ["http-server"]

[[sources]]
uri = "https://nginx.org/download/nginx-1.25.3.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"

[[sources]]
uri = "patches/fix-headers.patch"
sha256 = "SKIP"

[options]
strip = true
static = false
debug = false
ccache = true

[lifecycle.prepare]
executor = "shell"
dockyard = "strict"
script = """
cd nginx-${PART_VERSION}
patch -Np1 < ${FILES_DIR}/fix-headers.patch
"""

[lifecycle.configure]
executor = "shell"
dockyard = "strict"
env = { CFLAGS = "-O2 -pipe" }
script = """
cd nginx-${PART_VERSION}
./configure --prefix=/usr
"""

[lifecycle.compile]
executor = "shell"
dockyard = "strict"
script = """
cd nginx-${PART_VERSION}
make
"""

[lifecycle.check]
executor = "shell"
dockyard = "strict"
optional = true
script = """
cd nginx-${PART_VERSION}
make test
"""

[lifecycle.staging]
executor = "shell"
dockyard = "strict"
script = """
cd nginx-${PART_VERSION}
make DESTDIR=${PART_DIR} install
"""

[lifecycle.fabricate]
hooks.post_install = "useradd -r nginx 2>/dev/null || true"
hooks.post_upgrade = "systemctl reload nginx 2>/dev/null || true"
hooks.pre_remove = "systemctl stop nginx 2>/dev/null || true"
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.name, "nginx");
        assert_eq!(manifest.plan.url.as_deref(), Some("https://nginx.org"));
        assert_eq!(manifest.dependencies.runtime.len(), 3);
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
        assert_eq!(manifest.relations.provides, vec!["http-server"]);
        assert_eq!(manifest.sources.entries.len(), 2);
        assert!(manifest.options.strip);
        assert!(!manifest.options.static_);
        assert!(manifest.lifecycle.contains_key("check"));

        let scripts = manifest.install_scripts.as_ref().unwrap();
        assert!(scripts.post_install.is_some());
        assert!(scripts.pre_remove.is_some());

        let backup = manifest.backup.as_ref().unwrap();
        assert_eq!(backup.files.len(), 2);

        // New-style fabricate config
        match manifest.fabricate {
            Some(FabricateConfig::Single(ref output)) => {
                let hooks = output.hooks.as_ref().unwrap();
                assert!(hooks.post_install.is_some());
                assert!(hooks.pre_remove.is_some());
                assert_eq!(output.backup.as_ref().unwrap().len(), 2);
            }
            _ => panic!("expected Single fabricate config"),
        }
    }

    #[test]
    fn test_invalid_name() {
        let toml_str = r#"
[plan]
name = "Hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PlanManifest::parse(toml_str).is_err());
    }

    #[test]
    fn test_missing_name() {
        let toml_str = r#"
[plan]
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PlanManifest::parse(toml_str).is_err());
    }

    #[test]
    fn test_bad_version() {
        let toml_str = r#"
[plan]
name = "test"
version = "..."
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        assert!(PlanManifest::parse(toml_str).is_err());
    }

    #[test]
    fn test_archive_filename() {
        let toml_str = r#"
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(
            manifest.archive_filename(),
            "hello-1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_multi_packages() {
        let toml_str = r#"
[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[lifecycle.compile]
script = "make -j4"

[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[lifecycle.fabricate."libstdc++"]
description = "GNU C++ standard library"
script = """
install -Dm755 libstdc++.so ${PART_DIR}/usr/lib/libstdc++.so
"""

[lifecycle.fabricate."libstdc++".dependencies]
runtime = ["libgcc"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.fabricate {
            Some(FabricateConfig::Multi(ref pkgs)) => {
                assert_eq!(pkgs.len(), 2);
                let libstdcpp = pkgs.get("libstdc++").unwrap();
                assert_eq!(libstdcpp.description.as_deref(), Some("GNU C++ standard library"));
                assert_eq!(libstdcpp.dependencies.runtime, vec!["libgcc"]);

                // Test to_manifest
                let sub_manifest = libstdcpp.to_manifest("libstdc++", &manifest);
                assert_eq!(sub_manifest.plan.name, "libstdc++");
                assert_eq!(sub_manifest.plan.version, "14.2.0");
                assert_eq!(sub_manifest.plan.release, 1);
                assert_eq!(sub_manifest.plan.arch, "x86_64");
                assert_eq!(sub_manifest.plan.license, "GPL-3.0-or-later");
                assert_eq!(sub_manifest.plan.description, "GNU C++ standard library");
                assert_eq!(sub_manifest.dependencies.runtime, vec!["libgcc"]);
                assert_eq!(
                    sub_manifest.archive_filename(),
                    "libstdc++-14.2.0-1-x86_64.wright.tar.zst"
                );
            }
            _ => panic!("expected Multi fabricate config"),
        }
    }

    #[test]
    fn test_multi_package_inherits_overrides() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = "true"

[lifecycle.fabricate.test-doc]
description = "Documentation for test"
version = "1.0.0-doc"
arch = "any"
script = "true"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.fabricate {
            Some(FabricateConfig::Multi(ref pkgs)) => {
                let doc = pkgs.get("test-doc").unwrap();
                let doc_manifest = doc.to_manifest("test-doc", &manifest);
                assert_eq!(doc_manifest.plan.version, "1.0.0-doc");
                assert_eq!(doc_manifest.plan.arch, "any");
                assert_eq!(doc_manifest.plan.license, "MIT"); // inherited
            }
            _ => panic!("expected Multi fabricate config"),
        }
    }

    #[test]
    fn test_multi_package_missing_description() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.fabricate.test-lib]
script = "true"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("description is required"));
    }

    #[test]
    fn test_multi_package_invalid_name() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.fabricate.BadName]
description = "bad"
script = "true"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("invalid sub-package name"));
    }

    #[test]
    fn test_single_package_with_hooks_and_backup() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[lifecycle.fabricate]
hooks.pre_install = "echo pre"
hooks.post_install = "ldconfig"
hooks.pre_remove = "systemctl stop test"
backup = ["/etc/test.conf"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.fabricate {
            Some(FabricateConfig::Single(ref output)) => {
                let hooks = output.hooks.as_ref().unwrap();
                assert_eq!(hooks.pre_install.as_deref(), Some("echo pre"));
                assert_eq!(hooks.post_install.as_deref(), Some("ldconfig"));
                assert_eq!(hooks.pre_remove.as_deref(), Some("systemctl stop test"));
                assert_eq!(output.backup.as_ref().unwrap(), &["/etc/test.conf"]);
            }
            _ => panic!("expected Single fabricate config"),
        }
        // Legacy fields populated
        assert!(manifest.install_scripts.is_some());
        assert!(manifest.backup.is_some());
    }

    #[test]
    fn test_old_install_scripts_rejected() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.fabricate]
hooks.post_install = "ldconfig"

[install_scripts]
post_install = "ldconfig"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("unknown field `install_scripts`"));
    }

    #[test]
    fn test_old_backup_rejected() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[backup]
files = ["/etc/test.conf"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("unknown field `backup`"));
    }

    #[test]
    fn test_old_split_rejected() {
        let toml_str = r#"
[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[lifecycle.compile]
script = "make -j4"

[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[split."libstdc++"]
description = "GNU C++ standard library"

[split."libstdc++".dependencies]
runtime = ["libgcc"]

[split."libstdc++".lifecycle.part]
script = """
install -Dm755 libstdc++.so ${PART_DIR}/usr/lib/libstdc++.so
"""
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("unknown field `split`"));
    }

    #[test]
    fn test_main_package_in_multi_inherits_description() {
        let toml_str = r#"
[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[lifecycle.fabricate]
hooks.post_install = "ldconfig"

[lifecycle.fabricate."gcc-doc"]
description = "GCC documentation"
script = "true"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.fabricate {
            Some(FabricateConfig::Multi(ref pkgs)) => {
                let main = pkgs.get("gcc").unwrap();
                // Main package description is None — to_manifest will use parent's
                let main_manifest = main.to_manifest("gcc", &manifest);
                assert_eq!(main_manifest.plan.description, "The GNU Compiler Collection");
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn test_parse_mvp_section() {
        let toml_str = r#"
[plan]
name = "harfbuzz"
version = "8.0.0"
release = 1
description = "Text shaping library"
license = "MIT"
arch = "x86_64"

[dependencies]
link = ["freetype", "cairo", "glib"]

[mvp.dependencies]
link = ["freetype"]

[mvp.lifecycle.configure]
script = "meson setup build -Dglib=disabled"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        let mvp = manifest.mvp.as_ref().unwrap();
        let mvp_deps = mvp.dependencies.as_ref().unwrap();
        assert_eq!(mvp_deps.link.as_deref(), Some(&["freetype".to_string()][..]));
        assert!(mvp.lifecycle.contains_key("configure"));
        // Full deps unaffected
        assert_eq!(manifest.dependencies.link.len(), 3);
    }

    #[test]
    fn test_defaults() {
        let toml_str = r#"
[plan]
name = "minimal"
version = "1.0.0"
release = 1
description = "minimal package"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert!(manifest.dependencies.runtime.is_empty());
        assert!(manifest.dependencies.build.is_empty());
        assert!(manifest.sources.entries.is_empty());
        assert!(manifest.options.strip);
        assert!(manifest.lifecycle.is_empty());
        assert!(manifest.install_scripts.is_none());
        assert!(manifest.backup.is_none());
        assert!(!manifest.options.skip_fhs_check);
        assert_eq!(manifest.plan.epoch, 0);
    }

    #[test]
    fn test_skip_fhs_check_option() {
        let toml_str = r#"
[plan]
name = "kmod"
version = "1.0.0"
release = 1
description = "kernel module"
license = "GPL-2.0"
arch = "x86_64"

[options]
skip_fhs_check = true
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert!(manifest.options.skip_fhs_check);
    }

    // --- New v1.3.1 tests ---

    #[test]
    fn test_parse_relations_section() {
        let toml_str = r#"
[plan]
name = "nginx"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[relations]
replaces = ["old-nginx"]
conflicts = ["apache"]
provides = ["http-server"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.relations.replaces, vec!["old-nginx"]);
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
        assert_eq!(manifest.relations.provides, vec!["http-server"]);
    }

    #[test]
    fn test_parse_sources_array() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[sources]]
uri = "https://example.com/foo.tar.gz"
sha256 = "abc123"

[[sources]]
uri = "patches/fix.patch"
sha256 = "SKIP"

[[sources]]
uri = "git+https://github.com/foo/bar.git#v1.0"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.sources.entries.len(), 3);
        assert_eq!(manifest.sources.entries[0].uri, "https://example.com/foo.tar.gz");
        assert_eq!(manifest.sources.entries[0].sha256, "abc123");
        assert_eq!(manifest.sources.entries[1].sha256, "SKIP");
        // Git source without sha256 defaults to SKIP
        assert_eq!(manifest.sources.entries[2].sha256, "SKIP");

        // Test accessor methods
        let uris: Vec<&str> = manifest.sources.uris().collect();
        assert_eq!(uris.len(), 3);
        let sha256s: Vec<&str> = manifest.sources.sha256s().collect();
        assert_eq!(sha256s[0], "abc123");
    }

    #[test]
    fn test_parse_epoch() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
epoch = 2
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.epoch, 2);
        assert_eq!(
            manifest.archive_filename(),
            "test-2:1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_epoch_zero_omitted_from_filename() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
epoch = 0
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.epoch, 0);
        assert_eq!(
            manifest.archive_filename(),
            "test-1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_pre_install_hook() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.fabricate]
hooks.pre_install = "echo preparing"
hooks.post_install = "ldconfig"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.fabricate {
            Some(FabricateConfig::Single(ref output)) => {
                let hooks = output.hooks.as_ref().unwrap();
                assert_eq!(hooks.pre_install.as_deref(), Some("echo preparing"));
                assert_eq!(hooks.post_install.as_deref(), Some("ldconfig"));
            }
            _ => panic!("expected Single"),
        }
        let scripts = manifest.install_scripts.as_ref().unwrap();
        assert_eq!(scripts.pre_install.as_deref(), Some("echo preparing"));
    }

    #[test]
    fn test_old_sources_table_rejected() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[sources]
uris = ["https://example.com/foo.tar.gz", "patches/fix.patch"]
sha256 = ["abc123", "SKIP"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("sources must use [[sources]]"));
    }

    #[test]
    fn test_old_relations_in_dependencies_rejected() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = ["glibc"]
replaces = ["old-test"]
conflicts = ["other"]
provides = ["test-provider"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("unknown field `replaces`"));
    }

    #[test]
    fn test_parse_lifecycle_fabricate() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = "true"

[lifecycle.fabricate]
hooks.post_install = "ldconfig"
backup = ["/etc/test.conf"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.fabricate {
            Some(FabricateConfig::Single(ref output)) => {
                let hooks = output.hooks.as_ref().unwrap();
                assert_eq!(hooks.post_install.as_deref(), Some("ldconfig"));
                assert_eq!(output.backup.as_ref().unwrap(), &["/etc/test.conf"]);
            }
            _ => panic!("expected Single from [lifecycle.fabricate]"),
        }
    }

    #[test]
    fn test_lifecycle_package_rejected() {
        let toml_str = r#"
[plan]
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.package]
hooks.post_install = "ldconfig"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("[lifecycle.package] is no longer supported"));
    }

}
