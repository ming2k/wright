//! Typed representation and validation of `plan.toml`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

mod convert;
mod parse;
mod validate;

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
    /// part and checked as install-time warnings.
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

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct DiscardRule {
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum OutputConfig {
    /// Ordered list of outputs. At most one has `include = None` (the catch-all);
    /// all others carry explicit `include` patterns.  Include patterns across
    /// non-catch-all outputs must be mutually exclusive — a file matching more
    /// than one output is a build failure.  Any unclaimed file must match a
    /// discard rule or be packaged by the optional catch-all.
    Multi(Vec<(String, SubFabricateOutput)>),
}

// ---------------------------------------------------------------------------
// Archive metadata helper types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct DeployScripts {
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
/// Declared per-output in `[[output]]`.
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
    /// Optional filename to use for this source, in both the source cache and
    /// the work directory (defaults to the file's own basename in the work
    /// directory).
    pub r#as: Option<String>,
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
    pub metadata: PlanMetadata,
    /// Build dependencies — outputs that must be built and mounted into the
    /// isolation environment.
    pub build_deps: Vec<String>,
    /// Link dependencies — ABI-sensitive libraries that trigger reverse rebuilds.
    pub link_deps: Vec<String>,
    /// Runtime dependencies — libraries/tools required after installation.
    /// Aggregated from all [[output]] entries at parse time.
    pub runtime_deps: Vec<String>,
    pub relations: Relations,
    pub sources: Sources,
    pub options: PlanBuildOptions,
    pub pipeline: HashMap<String, PipelineStage>,
    pub pipeline_order: Option<PipelineOrder>,
    pub mvp: Option<PhaseConfig>,
    /// Fabricate output configuration.
    pub outputs: Option<OutputConfig>,
    /// Explicitly ignored staging files for multi-output slicing.
    pub discard: Vec<DiscardRule>,
    /// Derived archive metadata populated from outputs.
    pub deploy_scripts: Option<DeployScripts>,
    pub backup: Option<BackupConfig>,
    /// For sub-outputs, the original plan name. Used to write plan-level
    /// metadata into the part archive.
    pub source_plan: Option<String>,
    /// SHA-256 of the raw plan.toml bytes this manifest was loaded from
    /// (`mvp.toml` overlays excluded). Sealed into `.PARTINFO` `[provenance]`
    /// so the ledger can tie a part back to exact plan content (ADR-0023).
    /// `None` for manifests not loaded from a file.
    pub plan_checksum: Option<String>,
    /// Raw plan.toml text this manifest was loaded from (`mvp.toml` overlay
    /// excluded). Sealed into the part archive as `.PLANSRC` so the exact
    /// plan content that produced a part survives later edits to the plan
    /// source (ADR-0033). `None` for manifests not loaded from a file.
    pub plan_source: Option<String>,
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
pub struct PlanBuildOptions {
    #[serde(default, rename = "static")]
    pub static_: bool,
    #[serde(default)]
    pub debug: bool,
    #[serde(default = "default_true")]
    pub ccache: bool,
    /// Plan-wide environment variables injected into every pipeline stage.
    /// Per-stage `[pipeline.<stage>.env]` takes precedence over these.
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
    /// Skip ELF runtime dependency lint during sealing.
    /// Use for statically-linked plans or when the lint is a bottleneck
    /// in large batch forges. Errors caught here are still surfaced by
    /// `wright doctor` after deployment.
    #[serde(default)]
    pub skip_elf_lint: bool,
}

impl Default for PlanBuildOptions {
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
            skip_elf_lint: false,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Clone)]
pub struct PipelineStage {
    #[serde(default = "default_executor")]
    pub executor: String,
    /// Stage-specific isolation override. `None` inherits the executor or
    /// global build default and must remain distinguishable from `strict`.
    #[serde(default)]
    pub isolation: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub script: String,
}

fn default_executor() -> String {
    "shell".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct PipelineOrder {
    pub stages: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct PhaseConfig {
    /// Phase-specific build dependency overrides. Falls back to the top-level
    /// `build_deps` field when omitted.
    #[serde(default)]
    pub build_deps: Vec<String>,
    /// Phase-specific link dependency overrides. Falls back to the top-level
    /// `link_deps` field when omitted.
    #[serde(default)]
    pub link_deps: Vec<String>,
    /// Phase-specific runtime dependency overrides. Falls back to the
    /// aggregated `runtime_deps` field when omitted.
    #[serde(default)]
    pub runtime_deps: Vec<String>,
    #[serde(default)]
    pub pipeline: HashMap<String, PipelineStage>,
    #[serde(default)]
    pub pipeline_order: Option<PipelineOrder>,
}

impl PlanManifest {
    /// Get the archive filename for this part.
    /// Includes epoch only when > 0: `name-epoch:version-release-arch.wright.tar.zst`
    /// When version is absent, omits the version segment: `name-release-arch.wright.tar.zst`
    pub fn part_filename(&self) -> String {
        let ver = self.metadata.version.as_deref().unwrap_or("");
        if self.metadata.epoch > 0 {
            if ver.is_empty() {
                format!(
                    "{}-{}:{}-{}.wright.tar.zst",
                    self.metadata.name,
                    self.metadata.epoch,
                    self.metadata.release,
                    self.metadata.arch
                )
            } else {
                format!(
                    "{}-{}:{}-{}-{}.wright.tar.zst",
                    self.metadata.name,
                    self.metadata.epoch,
                    ver,
                    self.metadata.release,
                    self.metadata.arch
                )
            }
        } else if ver.is_empty() {
            format!(
                "{}-{}-{}.wright.tar.zst",
                self.metadata.name, self.metadata.release, self.metadata.arch
            )
        } else {
            format!(
                "{}-{}-{}-{}.wright.tar.zst",
                self.metadata.name, ver, self.metadata.release, self.metadata.arch
            )
        }
    }

    /// Name of the plan this manifest's archive belongs to.
    ///
    /// Sub-output manifests carry the originating plan in `source_plan`;
    /// single-output manifests are their own plan. Archives are sealed
    /// under `parts_dir/<plan_name>/` so several plans may ship same-named
    /// outputs without overwriting each other.
    pub fn plan_name(&self) -> &str {
        self.source_plan.as_deref().unwrap_or(&self.metadata.name)
    }

    /// Archive path relative to `parts_dir`: `<plan_name>/<part_filename()>`.
    pub fn part_rel_path(&self) -> PathBuf {
        PathBuf::from(self.plan_name()).join(self.part_filename())
    }

    /// Iterate over all outputs in declared order (multi-output mode only).
    pub fn output_parts(&self) -> impl Iterator<Item = (&str, &SubFabricateOutput)> {
        match self.outputs {
            Some(OutputConfig::Multi(ref parts)) => {
                Box::new(parts.iter().map(|(n, p)| (n.as_str(), p))) as Box<dyn Iterator<Item = _>>
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

    /// Get all dependency references with type labels.
    ///
    /// `build_deps` and `link_deps` are plan-level. `runtime_deps` is the
    /// aggregate of output-level install dependencies.
    pub fn all_dependencies(&self) -> Vec<(String, String)> {
        let mut all = Vec::new();
        for dep in &self.build_deps {
            all.push((dep.clone(), "build".to_string()));
        }
        for dep in &self.link_deps {
            all.push((dep.clone(), "link".to_string()));
        }
        for dep in &self.runtime_deps {
            all.push((dep.clone(), "runtime".to_string()));
        }
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_part_rel_path_uses_plan_subdirectory() {
        let toml_str = r#"
name = "hello"
version = "1.0.0"
release = 1
description = "test"
license = "MIT"
arch = "x86_64"
"#;
        let manifest = PlanManifest::parse(toml_str).unwrap();
        assert_eq!(manifest.plan_name(), "hello");
        assert_eq!(
            manifest.part_rel_path(),
            PathBuf::from("hello").join("hello-1.0.0-1-x86_64.wright.tar.zst")
        );
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
        assert_eq!(manifest.metadata.epoch, 2);
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
        assert_eq!(manifest.metadata.epoch, 0);
        assert_eq!(
            manifest.part_filename(),
            "test-1.0.0-1-x86_64.wright.tar.zst"
        );
    }
}
