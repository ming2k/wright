//! Group manifest: a declarative list of plans that form a coherent system.
//!
//! Unlike the pack format (which referenced pre-built archives), a group
//! is a pure manifest: it names plans, assumed externals, and optional
//! system configuration.  `wright launch` reads a group, resolves the
//! named plans, builds them, and installs the outputs into a target root.
//!
//! On-disk layout (single file):
//!
//! ```text
//! group.toml
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, WrightError};

pub const GROUP_MANIFEST_NAME: &str = "group.toml";

/// Top-level group manifest. Loaded from `group.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupManifest {
    pub group: GroupMeta,

    /// Externals the target is expected to provide.
    #[serde(default, rename = "assume")]
    pub assumes: Vec<GroupAssume>,

    /// Optional declarative system configuration applied after install.
    #[serde(default)]
    pub config: Option<GroupConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arch: String,
    /// Plan names that belong to this group.
    #[serde(default)]
    pub plans: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupAssume {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupConfig {
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
}

/// Read and parse a group manifest from a file path.
pub fn read_manifest(path: &Path) -> Result<GroupManifest> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| group_err(format!("failed to read {}: {}", path.display(), e)))?;
    parse_manifest(&raw)
}

/// Parse `group.toml` content.
pub fn parse_manifest(content: &str) -> Result<GroupManifest> {
    toml::from_str(content).map_err(|e| group_err(format!("invalid group.toml: {}", e)))
}

/// Search for a group manifest by name under the given groups directories.
///
/// Searches each directory in order, looking for `<dir>/<name>.toml`.
pub fn find_group_manifest(groups_dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    for groups_dir in groups_dirs {
        let path = groups_dir.join(format!("{}.toml", name));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Expand any `@group` references in the target list into their constituent
/// plan names.  Returns the fully-expanded list and, if any groups were
/// referenced, the merged assumptions and the last group's config.
pub fn expand_group_references(
    targets: Vec<String>,
    groups_dirs: &[PathBuf],
) -> Result<(Vec<String>, Vec<GroupAssume>, Option<GroupConfig>)> {
    let mut expanded = Vec::new();
    let mut all_assumes = Vec::new();
    let mut group_config: Option<GroupConfig> = None;

    for target in targets {
        if let Some(group_name) = target.strip_prefix('@') {
            let group_path = find_group_manifest(groups_dirs, group_name).ok_or_else(|| {
                group_err(format!(
                    "group '{}' not found in {}",
                    group_name,
                    groups_dirs
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            let manifest = read_manifest(&group_path)
                .map_err(|e| group_err(format!("read group {}: {}", group_path.display(), e)))?;
            expanded.extend(manifest.group.plans);
            all_assumes.extend(manifest.assumes);
            if manifest.config.is_some() {
                group_config = manifest.config;
            }
        } else {
            expanded.push(target);
        }
    }

    Ok((expanded, all_assumes, group_config))
}

fn group_err(msg: String) -> WrightError {
    WrightError::PartError(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let m = parse_manifest(
            r#"
[group]
name = "core"
version = "1"
description = "core"
arch = "x86_64"

plans = ["glibc", "bash"]
"#,
        )
        .unwrap();
        assert_eq!(m.group.name, "core");
        assert_eq!(m.group.plans, vec!["glibc", "bash"]);
    }

    #[test]
    fn parses_full_manifest() {
        let m = parse_manifest(
            r#"
[group]
name = "base"
version = "2026.05"
description = "Base system"
arch = "x86_64"

plans = ["glibc", "bash", "coreutils"]

[[assume]]
name = "linux"
version = "6.12.0"

[config]
hostname = "wright"
timezone = "UTC"
locale = "en_US.UTF-8"
services = ["sshd"]
"#,
        )
        .unwrap();
        assert_eq!(m.group.name, "base");
        assert_eq!(m.group.plans.len(), 3);
        assert_eq!(m.assumes.len(), 1);
        let cfg = m.config.unwrap();
        assert_eq!(cfg.hostname.as_deref(), Some("wright"));
        assert_eq!(cfg.services, vec!["sshd".to_string()]);
    }
}
