use std::collections::HashMap;

use serde::Deserialize;

use crate::error::{Result, WrightError};

mod convert;
mod parse;

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

/// Main output metadata.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct FabricateOutput {
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub backup: Option<Vec<String>>,
}

/// Additional output mode.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SubFabricateOutput {
    #[serde(default)]
    pub description: Option<String>,
    pub version: Option<String>,
    pub release: Option<u32>,
    pub arch: Option<String>,
    pub license: Option<String>,
    /// Runtime dependencies for this specific output. Recorded in the binary
    /// part and enforced at install time.
    #[serde(default)]
    pub runtime_deps: Vec<String>,
    /// Parts that this output replaces (automatic uninstall on install).
    #[serde(default)]
    pub replaces: Vec<String>,
    /// Parts that cannot coexist with this output.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Virtual part names this output satisfies.
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    #[serde(default)]
    pub hooks: Option<FabricateHooks>,
    #[serde(default)]
    pub backup: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub enum OutputConfig {
    Single(FabricateOutput),
    /// Ordered list of outputs. Exactly one has `include = None` (the catch-all);
    /// all others carry explicit `include` patterns. Non-catch-all outputs are
    /// processed in declared order, moving matching files out of the staging dir.
    /// The catch-all keeps whatever remains — no move needed.
    Multi(Vec<(String, SubFabricateOutput)>),
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

/// Part relations — install-time metadata describing how a part interacts with
/// other parts.
///
/// - **`replaces`**: Automatic migration — installing this part silently removes
///   the listed parts first. Use for renames and merges.
/// - **`conflicts`**: Mutual exclusion — installation is refused while a
///   conflicting part is present. Use when two parts cannot coexist.
/// - **`provides`**: Virtual names — allows this part to satisfy dependencies on
///   an abstract capability (e.g. `http-server`).
///
/// Declared per-output in `[output]` (main part) or `[output.<name>]` (sub-part).
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
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Http(HttpSource),
    Git(GitSource),
    Local(LocalSource),
}

#[derive(Debug, Deserialize, Clone)]
pub struct HttpSource {
    pub url: String,
    #[serde(default = "default_skip")]
    pub sha256: String,
    /// Optional local filename to use for the downloaded source.
    pub r#as: Option<String>,
    /// Optional subdirectory under WORKDIR to extract/copy this source into.
    pub extract_to: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GitSource {
    pub url: String,
    pub r#ref: Option<String>,
    /// Git fetch depth. Defaults to 1 (shallow clone). Set to `null` or omit
    /// to use full clone when needed (e.g. for arbitrary commit hashes).
    #[serde(default = "default_git_depth")]
    pub depth: Option<u32>,
    /// Optional subdirectory under WORKDIR to extract/copy this source into.
    pub extract_to: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LocalSource {
    pub path: String,
    /// Optional subdirectory under WORKDIR to extract/copy this source into.
    pub extract_to: Option<String>,
}

fn default_skip() -> String {
    "SKIP".to_string()
}

fn default_git_depth() -> Option<u32> {
    Some(1)
}

#[derive(Debug, Clone)]
pub struct PlanManifest {
    pub plan: PlanMetadata,
    /// Build dependencies — tools needed during compilation.
    pub build_deps: Vec<String>,
    /// Link dependencies — ABI-sensitive libraries that trigger reverse rebuilds.
    pub link_deps: Vec<String>,
    /// Runtime dependencies — libraries/tools required after installation.
    /// Aggregated from all [[output]] entries at parse time.
    pub runtime_deps: Vec<String>,
    pub relations: Relations,
    pub sources: Sources,
    pub options: BuildOptions,
    pub lifecycle: HashMap<String, LifecycleStage>,
    pub lifecycle_order: Option<LifecycleOrder>,
    pub mvp: Option<PhaseConfig>,
    /// Fabricate output configuration.
    pub outputs: Option<OutputConfig>,
    /// Derived archive metadata populated from `fabricate`.
    pub install_scripts: Option<InstallScripts>,
    pub backup: Option<BackupConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PlanMetadata {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
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


#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub entries: Vec<Source>,
}

impl Sources {}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BuildOptions {
    #[serde(default, rename = "static")]
    pub static_: bool,
    #[serde(default)]
    pub debug: bool,
    #[serde(default = "default_true")]
    pub ccache: bool,
    /// Plan-wide environment variables injected into every lifecycle stage.
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
    /// Set to `true` only for parts with a deliberate reason to install
    /// outside the standard FHS paths (e.g. kernel modules, legacy compat layers).
    #[serde(default)]
    pub skip_fhs_check: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
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
    #[serde(default = "default_isolation_level")]
    pub isolation: String,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub script: String,
}

fn default_executor() -> String {
    "shell".to_string()
}

fn default_isolation_level() -> String {
    "strict".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct LifecycleOrder {
    pub stages: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct PhaseConfig {
    /// Phase-specific build dependency overrides. Falls back to the top-level
    /// `build` field when omitted.
    #[serde(default)]
    pub build: Vec<String>,
    /// Phase-specific link dependency overrides. Falls back to the top-level
    /// `link` field when omitted.
    #[serde(default)]
    pub link: Vec<String>,
    #[serde(default)]
    pub lifecycle: HashMap<String, LifecycleStage>,
    #[serde(default)]
    pub lifecycle_order: Option<LifecycleOrder>,
}

impl PlanManifest {
    pub fn validate(&self) -> Result<()> {
        let name_re = regex::Regex::new(r"^[a-z0-9][a-z0-9_+.-]*$").unwrap();
        if !name_re.is_match(&self.plan.name) {
            return Err(WrightError::ValidationError(format!(
                "invalid part name '{}': must match [a-z0-9][a-z0-9_+.-]*",
                self.plan.name
            )));
        }
        if self.plan.name.len() > 64 {
            return Err(WrightError::ValidationError(
                "part name must be at most 64 characters".to_string(),
            ));
        }

        // Validate version parses if present
        if let Some(ref ver) = self.plan.version {
            crate::part::version::Version::parse(ver)?;
        }

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
                    stages
                        .iter()
                        .filter(|s| !["fetch", "verify", "extract"].contains(s))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }

        // Each source entry is self-contained (uri + sha256), no positional check needed

        // Validate fabricate config
        if let Some(ref part) = self.outputs {
            match part {
                OutputConfig::Multi(ref parts) => {
                    let catchall_count = parts.iter().filter(|(_, s)| s.include.is_none()).count();
                    if catchall_count == 0 {
                        return Err(WrightError::ValidationError(
                            "multi-output plans must have exactly one catch-all output (an [output.<name>] with no 'include')".to_string(),
                        ));
                    }
                    if catchall_count > 1 {
                        return Err(WrightError::ValidationError(
                            "multiple outputs have no 'include'; exactly one catch-all is allowed".to_string(),
                        ));
                    }
                    for (sub_name, sub_part) in parts {
                        if !name_re.is_match(sub_name) {
                            return Err(WrightError::ValidationError(format!(
                                "invalid output name '{}': must match [a-z0-9][a-z0-9_+.-]*",
                                sub_name
                            )));
                        }
                        if matches!(&sub_part.include, Some(v) if v.is_empty()) {
                            return Err(WrightError::ValidationError(format!(
                                "output '{}': include = [] is invalid; list patterns or omit include for the catch-all",
                                sub_name
                            )));
                        }
                        // Non-catch-all outputs must have a description
                        if sub_part.include.is_some() && sub_part.description.is_none() {
                            return Err(WrightError::ValidationError(format!(
                                "output '{}': description is required for non-catch-all outputs",
                                sub_name
                            )));
                        }
                        if let Some(ref ver) = sub_part.version {
                            crate::part::version::Version::parse(ver)?;
                        }
                        if let Some(ref rel) = sub_part.release {
                            if *rel == 0 {
                                return Err(WrightError::ValidationError(format!(
                                    "output '{}': release must be >= 1",
                                    sub_name
                                )));
                            }
                        }
                    }
                }
                OutputConfig::Single(_) => {}
            }
        }

        // Validate individual lifecycle stage isolation
        for (name, stage) in &self.lifecycle {
            if let Err(e) = stage.isolation.parse::<crate::isolation::IsolationLevel>() {
                return Err(WrightError::ValidationError(format!(
                    "stage '{}': invalid isolation level '{}': {}",
                    name, stage.isolation, e
                )));
            }
        }

        // Validate plan-level deps (build/link): no empty strings, no duplicates
        let dep_kinds = [
            ("build", &self.build_deps),
            ("link", &self.link_deps),
        ];
        for (kind, deps) in &dep_kinds {
            let mut seen = std::collections::HashSet::new();
            for dep in *deps {
                let trimmed = dep.trim();
                if trimmed.is_empty() {
                    return Err(WrightError::ValidationError(format!(
                        "{} contains an empty entry",
                        kind
                    )));
                }
                if !seen.insert(trimmed) {
                    return Err(WrightError::ValidationError(format!(
                        "{} contains duplicate entry '{}'",
                        kind, trimmed
                    )));
                }
            }
        }

        Ok(())
    }

    /// Get the archive filename for this part.
    /// Includes epoch only when > 0: `name-epoch:version-release-arch.wright.tar.zst`
    /// When version is absent, omits the version segment: `name-release-arch.wright.tar.zst`
    pub fn part_filename(&self) -> String {
        let ver = self.plan.version.as_deref().unwrap_or("");
        if self.plan.epoch > 0 {
            if ver.is_empty() {
                format!(
                    "{}-{}:{}-{}.wright.tar.zst",
                    self.plan.name, self.plan.epoch, self.plan.release, self.plan.arch
                )
            } else {
                format!(
                    "{}-{}:{}-{}-{}.wright.tar.zst",
                    self.plan.name, self.plan.epoch, ver, self.plan.release, self.plan.arch
                )
            }
        } else if ver.is_empty() {
            format!(
                "{}-{}-{}.wright.tar.zst",
                self.plan.name, self.plan.release, self.plan.arch
            )
        } else {
            format!(
                "{}-{}-{}-{}.wright.tar.zst",
                self.plan.name, ver, self.plan.release, self.plan.arch
            )
        }
    }

    /// Iterate over all outputs in declared order (multi-output mode only).
    pub fn output_parts(&self) -> impl Iterator<Item = (&str, &SubFabricateOutput)> {
        match self.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                Box::new(parts.iter().map(|(n, p)| (n.as_str(), p)))
                    as Box<dyn Iterator<Item = _>>
            }
            _ => Box::new(std::iter::empty()),
        }
    }

    /// Iterate over non-catch-all outputs (those with explicit `include` patterns).
    pub fn non_catchall_parts(&self) -> impl Iterator<Item = (&str, &SubFabricateOutput)> {
        self.output_parts().filter(|(_, p)| p.include.is_some())
    }

    /// Return the catch-all output (the one with no `include`), if in multi-output mode.
    pub fn catchall_part(&self) -> Option<(&str, &SubFabricateOutput)> {
        match self.outputs {
            Some(OutputConfig::Multi(ref parts)) => parts
                .iter()
                .find(|(_, p)| p.include.is_none())
                .map(|(n, p)| (n.as_str(), p)),
            _ => None,
        }
    }

    /// Get all plan-level dependencies (build, link) with their type labels.
    pub fn all_dependencies(&self) -> Vec<(String, String)> {
        let mut all = Vec::new();
        for dep in &self.build_deps {
            all.push((dep.clone(), "build".to_string()));
        }
        for dep in &self.link_deps {
            all.push((dep.clone(), "link".to_string()));
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hello_fixture() {
        let toml_str = r#"
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test part"
license = "MIT"
arch = "x86_64"

build = ["gcc"]

[lifecycle.prepare]
executor = "shell"
isolation = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.compile]
executor = "shell"
isolation = "none"
script = """
gcc -o hello hello.c
"""

[lifecycle.staging]
executor = "shell"
isolation = "none"
script = """
install -Dm755 hello ${PART_DIR}/usr/bin/hello
"""
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.name, "hello");
        assert_eq!(manifest.plan.version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.plan.release, 1);
        assert_eq!(manifest.plan.arch, "x86_64");
        assert_eq!(manifest.plan.epoch, 0);
        assert_eq!(manifest.build_deps, vec!["gcc"]);
        assert!(manifest.lifecycle.contains_key("prepare"));
        assert!(manifest.lifecycle.contains_key("compile"));
        assert!(manifest.lifecycle.contains_key("staging"));
    }

    #[test]
    fn test_parse_full_featured() {
        let toml_str = r#"
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Test <test@test.com>"

build = ["perl", "gcc", "make"]
link = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]

[[sources]]
type = "http"
url = "https://nginx.org/download/nginx-1.25.3.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"

[[sources]]
type = "local"
path = "patches/fix-headers.patch"

[options]
static = false
debug = false
ccache = true

[lifecycle.prepare]
executor = "shell"
isolation = "strict"
script = """
cd ${BUILD_DIR}
patch -Np1 < ${WORKDIR}/fix-headers.patch
"""

[lifecycle.configure]
executor = "shell"
isolation = "strict"
env = { CFLAGS = "-O2 -pipe" }
script = """
cd ${BUILD_DIR}
./configure --prefix=/usr
"""

[lifecycle.compile]
executor = "shell"
isolation = "strict"
script = """
cd ${BUILD_DIR}
make
"""

[lifecycle.check]
executor = "shell"
isolation = "strict"
optional = true
script = """
cd ${BUILD_DIR}
make test
"""

[lifecycle.staging]
executor = "shell"
isolation = "strict"
script = """
cd ${BUILD_DIR}
make DESTDIR=${PART_DIR} install
"""

[hooks]
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"

[output]
conflicts = ["apache"]
provides = ["http-server"]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.name, "nginx");
        assert_eq!(manifest.plan.url.as_deref(), Some("https://nginx.org"));
        assert!(manifest.runtime_deps.is_empty());
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
        assert_eq!(manifest.relations.provides, vec!["http-server"]);
        assert_eq!(manifest.sources.entries.len(), 2);
        assert!(!manifest.options.static_);
        assert!(manifest.lifecycle.contains_key("check"));

        let scripts = manifest.install_scripts.as_ref().unwrap();
        assert!(scripts.post_install.is_some());
        assert!(scripts.pre_remove.is_some());

        let backup = manifest.backup.as_ref().unwrap();
        assert_eq!(backup.files.len(), 2);

        // New-style fabricate config
        match manifest.outputs {
            Some(OutputConfig::Single(ref output)) => {
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
    fn test_part_filename() {
        let toml_str = r#"
name = "hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(
            manifest.part_filename(),
            "hello-1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_multi_packages() {
        let toml_str = r#"
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

[[output]]
name = "gcc"

[[output]]
name = "libstdc++"
description = "GNU C++ standard library"
include = ["/usr/lib/libstdc.*"]
runtime_deps = ["libgcc"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                assert_eq!(parts.len(), 2);
                let (_, libstdcpp) = parts.iter().find(|(n, _)| n == "libstdc++").unwrap();
                assert_eq!(
                    libstdcpp.description.as_deref(),
                    Some("GNU C++ standard library")
                );
                assert_eq!(libstdcpp.runtime_deps, vec!["libgcc"]);

                let sub_manifest = libstdcpp.to_manifest("libstdc++", &manifest);
                assert_eq!(sub_manifest.plan.name, "libstdc++");
                assert_eq!(sub_manifest.plan.version.as_deref(), Some("14.2.0"));
                assert_eq!(sub_manifest.plan.release, 1);
                assert_eq!(sub_manifest.plan.arch, "x86_64");
                assert_eq!(sub_manifest.plan.license, "GPL-3.0-or-later");
                assert_eq!(sub_manifest.plan.description, "GNU C++ standard library");
                assert_eq!(sub_manifest.runtime_deps, vec!["libgcc"]);
                assert_eq!(
                    sub_manifest.part_filename(),
                    "libstdc++-14.2.0-1-x86_64.wright.tar.zst"
                );
            }
            _ => panic!("expected Multi fabricate config"),
        }
    }

    #[test]
    fn test_multi_package_sub_part_relations() {
        let toml_str = r#"
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP server"
license = "BSD-2-Clause"
arch = "x86_64"


[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[[output]]
name = "nginx"
conflicts = ["apache"]
provides = ["http-server"]

[[output]]
name = "nginx-doc"
description = "Nginx documentation files"
provides = ["nginx-documentation"]
include = ["/usr/share/doc/.*"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.relations.conflicts, vec!["apache"]);
        assert_eq!(manifest.relations.provides, vec!["http-server"]);

        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                let (_, main) = parts.iter().find(|(n, _)| n == "nginx").unwrap();
                let main_manifest = main.to_manifest("nginx", &manifest);
                assert_eq!(main_manifest.relations.conflicts, vec!["apache"]);
                assert_eq!(main_manifest.relations.provides, vec!["http-server"]);

                let (_, doc) = parts.iter().find(|(n, _)| n == "nginx-doc").unwrap();
                let doc_manifest = doc.to_manifest("nginx-doc", &manifest);
                assert_eq!(doc_manifest.relations.provides, vec!["nginx-documentation"]);
                assert!(doc_manifest.relations.conflicts.is_empty());
            }
            _ => panic!("expected Multi fabricate config"),
        }
    }

    #[test]
    fn test_multi_package_inherits_overrides() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[lifecycle.staging]
script = "true"

[[output]]
name = "test"

[[output]]
name = "test-doc"
description = "Documentation for test"
version = "1.0.0-doc"
arch = "any"
include = ["/usr/share/doc/.*"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                let (_, doc) = parts.iter().find(|(n, _)| n == "test-doc").unwrap();
                let doc_manifest = doc.to_manifest("test-doc", &manifest);
                assert_eq!(doc_manifest.plan.version.as_deref(), Some("1.0.0-doc"));
                assert_eq!(doc_manifest.plan.arch, "any");
                assert_eq!(doc_manifest.plan.license, "MIT"); // inherited
            }
            _ => panic!("expected Multi fabricate config"),
        }
    }

    #[test]
    fn test_multi_package_missing_description() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"

[[output]]
name = "test-lib"
include = ["/usr/lib/.*"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("description is required"));
    }

    #[test]
    fn test_multi_package_invalid_name() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"

[[output]]
name = "BadName"
description = "bad"
include = ["/usr/bin/.*"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("invalid output name"));
    }

    #[test]
    fn test_multi_output_include_empty_error() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"

[[output]]
name = "test-lib"
description = "test lib"
include = []
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("include = []"));
    }

    #[test]
    fn test_multi_output_no_catchall_error() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"
include = ["/usr/bin/.*"]

[[output]]
name = "test-lib"
description = "test lib"
include = ["/usr/lib/.*"]
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("catch-all"));
    }

    #[test]
    fn test_multi_output_multiple_catchall_error() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"


[[output]]
name = "test"

[[output]]
name = "test-lib"
description = "test lib"
"#;
        let err = PlanManifest::parse(toml_str).unwrap_err();
        assert!(err.to_string().contains("catch-all"));
    }

    #[test]
    fn test_single_output_with_output_section_ok() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[output]]
name = "test-lib"
description = "test lib"
runtime_deps = ["openssl"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan.name, "test");
        assert_eq!(manifest.runtime_deps, vec!["openssl"]);
        match manifest.outputs {
            Some(OutputConfig::Multi(parts)) => {
                assert_eq!(parts.len(), 1);
                assert_eq!(parts[0].0, "test-lib");
                assert_eq!(parts[0].1.runtime_deps, vec!["openssl"]);
            }
            _ => panic!("expected Multi output config"),
        }
    }

    #[test]
    fn test_multi_output_order_preserved() {
        let toml_str = r#"
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"


[[output]]
name = "gcc-libs"
description = "GCC runtime libraries"
include = ["/usr/lib/.*\\.so.*"]

[[output]]
name = "gcc-dev"
description = "GCC development files"
include = ["/usr/include/.*", "/usr/lib/.*\\.a"]

[[output]]
name = "gcc"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                assert_eq!(parts.len(), 3);
                assert_eq!(parts[0].0, "gcc-libs");
                assert_eq!(parts[1].0, "gcc-dev");
                assert_eq!(parts[2].0, "gcc");
                // gcc is the catch-all
                assert!(parts[2].1.include.is_none());
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn test_single_package_with_hooks_and_backup() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[hooks]
pre_install = "echo pre"
post_install = "ldconfig"
pre_remove = "systemctl stop test"

[output]
backup = ["/etc/test.conf"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Single(ref output)) => {
                let hooks = output.hooks.as_ref().unwrap();
                assert_eq!(hooks.pre_install.as_deref(), Some("echo pre"));
                assert_eq!(hooks.post_install.as_deref(), Some("ldconfig"));
                assert_eq!(hooks.pre_remove.as_deref(), Some("systemctl stop test"));
                assert_eq!(output.backup.as_ref().unwrap(), &["/etc/test.conf"]);
            }
            _ => panic!("expected Single fabricate config"),
        }
        assert!(manifest.install_scripts.is_some());
        assert!(manifest.backup.is_some());
    }

    #[test]
    fn test_main_package_in_multi_inherits_description() {
        let toml_str = r#"
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"


[lifecycle.staging]
script = "make DESTDIR=${PART_DIR} install"

[[output]]
name = "gcc"

[[output]]
name = "gcc-doc"
description = "GCC documentation"
include = ["/usr/share/doc/.*"]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                let (_, main) = parts.iter().find(|(n, _)| n == "gcc").unwrap();
                let main_manifest = main.to_manifest("gcc", &manifest);
                assert_eq!(
                    main_manifest.plan.description,
                    "The GNU Compiler Collection"
                );
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn test_from_file_loads_sibling_mvp_toml() {
        let dir = tempfile::tempdir().unwrap();
        let plan_path = dir.path().join("plan.toml");
        let mvp_path = dir.path().join("mvp.toml");

        std::fs::write(
            &plan_path,
            r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();

        std::fs::write(
            &mvp_path,
            r#"
build = ["gcc", "make"]

[lifecycle.configure]
script = "echo mvp"
"#,
        )
        .unwrap();

        let manifest = PlanManifest::from_file(&plan_path).unwrap();
        let mvp = manifest.mvp.as_ref().unwrap();
        assert_eq!(mvp.build, vec!["gcc", "make"]);
        assert_eq!(
            mvp.lifecycle
                .get("configure")
                .map(|stage| stage.script.as_str()),
            Some("echo mvp")
        );
    }

    #[test]
    fn test_defaults() {
        let toml_str = r#"
name = "minimal"
version = "1.0.0"
release = 1
description = "minimal part"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert!(manifest.runtime_deps.is_empty());
        assert!(manifest.build_deps.is_empty());
        assert!(manifest.sources.entries.is_empty());
        assert!(manifest.lifecycle.is_empty());
        assert!(manifest.install_scripts.is_none());
        assert!(manifest.backup.is_none());
        assert!(!manifest.options.skip_fhs_check);
        assert_eq!(manifest.plan.epoch, 0);
    }

    #[test]
    fn test_skip_fhs_check_option() {
        let toml_str = r#"
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

    #[test]
    fn test_parse_output_relations() {
        let toml_str = r#"
name = "nginx"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[output]
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
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.com/foo.tar.gz"
sha256 = "abc123"

[[sources]]
type = "local"
path = "patches/fix.patch"

[[sources]]
type = "git"
url = "https://github.com/foo/bar.git"
ref = "v1.0"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.sources.entries.len(), 3);

        if let Source::Http(http) = &manifest.sources.entries[0] {
            assert_eq!(http.url, "https://example.com/foo.tar.gz");
            assert_eq!(http.sha256, "abc123");
        } else {
            panic!("Expected Http source");
        }

        if let Source::Local(local) = &manifest.sources.entries[1] {
            assert_eq!(local.path, "patches/fix.patch");
        } else {
            panic!("Expected Local source");
        }

        if let Source::Git(git) = &manifest.sources.entries[2] {
            assert_eq!(git.url, "https://github.com/foo/bar.git");
            assert_eq!(git.r#ref, Some("v1.0".to_string()));
        } else {
            panic!("Expected Git source");
        }
    }

    #[test]
    fn test_parse_epoch() {
        let toml_str = r#"
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
            manifest.part_filename(),
            "test-2:1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_epoch_zero_omitted_from_filename() {
        let toml_str = r#"
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
            manifest.part_filename(),
            "test-1.0.0-1-x86_64.wright.tar.zst"
        );
    }

    #[test]
    fn test_parse_pre_install_hook() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[hooks]
pre_install = "echo preparing"
post_install = "ldconfig"

[output]
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        match manifest.outputs {
            Some(OutputConfig::Single(ref output)) => {
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
    fn test_parse_lifecycle_fabricate() {
        let toml_str = r#"
name = "test"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = "true"

[lifecycle.outputs]
script = "strip ${PART_DIR}/usr/bin/test"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(
            manifest
                .lifecycle
                .get("fabricate")
                .map(|stage| stage.script.as_str()),
            Some("strip ${PART_DIR}/usr/bin/test")
        );
    }
}
