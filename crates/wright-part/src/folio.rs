//! Folio manifest — a declarative list of plans that form a coherent system.
//!
//! A folio is a pure manifest: it names plans, externals assumed to be on
//! the target system, and optional post-launch hooks.  `wright launch`
//! reads a folio, resolves the named plans, builds them, and installs the
//! outputs into a target root.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, WrightError};

/// Top-level folio manifest. Loaded from `<name>.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolioManifest {
    pub folio: FolioMeta,

    /// Externals the target is expected to provide.
    #[serde(default, rename = "provide")]
    pub provides: Vec<FolioProvide>,

    /// Hooks executed after all plans are built and deployed.
    #[serde(default, rename = "hook")]
    pub hooks: Vec<Hook>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolioMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub plans: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolioProvide {
    pub name: String,
    pub version: String,
}

/// A shell script executed at a fixed stage of `wright launch`.
///
/// Hooks run on the host with both `$WRIGHT_ROOT` and `$ROOT` set to the
/// target root path.  Scripts run unsandboxed with the same privileges as
/// the `wright` process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub stage: HookStage,
    pub script: String,
}

/// Supported hook stages. Unknown values are a parse error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookStage {
    /// Runs after every plan in the folio has been built, sealed, and
    /// deployed into the target root.
    PostLaunch,
}

impl FolioManifest {
    /// Read and parse a folio manifest from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| err(format!("read {}: {}", path.display(), e)))?;
        Self::parse(&raw).map_err(|e| err(format!("{}: {}", path.display(), e)))
    }

    /// Parse a folio manifest from a string.
    pub fn parse(content: &str) -> Result<Self> {
        toml::from_str(content).map_err(|e| err(format!("invalid folio manifest: {e}")))
    }
}

/// Locate a folio manifest by bare name across the given search dirs.
///
/// Returns the first `<dir>/<name>.toml` that exists.  Search order matches
/// the order of `dirs`.
pub fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        let path = dir.join(format!("{name}.toml"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// The result of expanding a target list against the available folios.
#[derive(Debug, Default, Clone)]
pub struct Expansion {
    /// Plan names to build, in target-list order.
    pub plans: Vec<String>,
    /// Provides collected from every referenced folio.
    pub provides: Vec<FolioProvide>,
    /// Hooks collected from every referenced folio.
    pub hooks: Vec<Hook>,
    /// Paths of folio files that were resolved (for syncing into a target).
    pub referenced: Vec<PathBuf>,
}

/// Expand `@folio` references in `targets` into their constituent plan names.
///
/// Non-prefixed entries pass through unchanged.  Entries prefixed with `@`
/// are looked up in `folio_dirs` and replaced with the folio's `plans`,
/// while their `[[provide]]` and `[[hook]]` blocks are accumulated.
pub fn expand(targets: &[String], folio_dirs: &[PathBuf]) -> Result<Expansion> {
    let mut out = Expansion::default();
    for target in targets {
        match target.strip_prefix('@') {
            Some(name) => {
                let path = find(name, folio_dirs).ok_or_else(|| {
                    err(format!(
                        "folio '{}' not found in: {}",
                        name,
                        folio_dirs
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?;
                let manifest = FolioManifest::load(&path)?;
                out.plans.extend(manifest.folio.plans);
                out.provides.extend(manifest.provides);
                out.hooks.extend(manifest.hooks);
                out.referenced.push(path);
            }
            None => out.plans.push(target.clone()),
        }
    }
    Ok(out)
}

fn err(msg: String) -> WrightError {
    WrightError::PartError(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let m = FolioManifest::parse(
            r#"
[folio]
name = "core"
version = "1"
plans = ["glibc", "bash"]
"#,
        )
        .unwrap();
        assert_eq!(m.folio.name, "core");
        assert_eq!(m.folio.plans, vec!["glibc", "bash"]);
        assert!(m.provides.is_empty());
        assert!(m.hooks.is_empty());
    }

    #[test]
    fn parses_full_manifest() {
        let m = FolioManifest::parse(
            r#"
[folio]
name = "base"
version = "2026.05"
description = "Base system"
plans = ["glibc", "bash", "coreutils"]

[[provide]]
name = "linux"
version = "6.12.0"

[[hook]]
stage = "post-launch"
script = "ln -s /etc/sv/sshd $ROOT/var/service/sshd"
"#,
        )
        .unwrap();
        assert_eq!(m.folio.plans.len(), 3);
        assert_eq!(m.provides.len(), 1);
        assert_eq!(m.hooks.len(), 1);
        assert_eq!(m.hooks[0].stage, HookStage::PostLaunch);
    }

    #[test]
    fn rejects_unknown_top_level_table() {
        let err = FolioManifest::parse(
            r#"
[folio]
name = "x"
version = "1"

[config]
hostname = "old"
"#,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("config"), "{err}");
    }

    #[test]
    fn rejects_unknown_hook_stage() {
        let err = FolioManifest::parse(
            r#"
[folio]
name = "x"
version = "1"

[[hook]]
stage = "pre-launch"
script = "true"
"#,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("pre-launch"), "{err}");
    }

    #[test]
    fn expands_plain_and_folio_targets() {
        use std::fs;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        fs::write(
            dir.join("core.toml"),
            r#"
[folio]
name = "core"
version = "1"
plans = ["a", "b"]

[[provide]]
name = "linux"
version = "6.0"
"#,
        )
        .unwrap();

        let exp = expand(
            &["@core".to_string(), "c".to_string()],
            std::slice::from_ref(&dir),
        )
        .unwrap();
        assert_eq!(exp.plans, vec!["a", "b", "c"]);
        assert_eq!(exp.provides.len(), 1);
        assert_eq!(exp.referenced, vec![dir.join("core.toml")]);
    }
}
