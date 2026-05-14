//! Folio manifest: a declarative list of plans that form a coherent system.
//!
//! Unlike the pack format (which referenced pre-built archives), a folio
//! is a pure manifest: it names plans, assumed externals, and optional
//! system configuration.  `wright launch` reads a folio, resolves the
//! named plans, builds them, and installs the outputs into a target root.
//!
//! On-disk layout (single file):
//!
//! ```text
//! folio.toml
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, WrightError};

pub const FOLIO_MANIFEST_NAME: &str = "folio.toml";

/// Top-level folio manifest. Loaded from `folio.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolioManifest {
    pub folio: FolioMeta,

    /// Externals the target is expected to provide.
    #[serde(default, rename = "provide")]
    pub provides: Vec<FolioProvide>,

    /// Optional declarative system configuration applied after install.
    #[serde(default)]
    pub config: Option<FolioConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolioMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arch: String,
    /// Plan names that belong to this folio.
    #[serde(default)]
    pub plans: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolioProvide {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FolioConfig {
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
}

/// Read and parse a folio manifest from a file path.
pub fn read_manifest(path: &Path) -> Result<FolioManifest> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| folio_err(format!("failed to read {}: {}", path.display(), e)))?;
    parse_manifest(&raw)
}

/// Parse `folio.toml` content.
pub fn parse_manifest(content: &str) -> Result<FolioManifest> {
    toml::from_str(content).map_err(|e| folio_err(format!("invalid folio.toml: {}", e)))
}

/// Search for a folio manifest by name under the given folios directories.
///
/// Searches each directory in order, looking for `<dir>/<name>.toml`.
pub fn find_folio_manifest(folios_dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    for folios_dir in folios_dirs {
        let path = folios_dir.join(format!("{}.toml", name));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Expand any `@folio` references in the target list into their constituent
/// plan names.  Returns the fully-expanded list and, if any folios were
/// referenced, the merged assumptions and the last folio's config.
pub fn expand_folio_references(
    targets: Vec<String>,
    folios_dirs: &[PathBuf],
) -> Result<(Vec<String>, Vec<FolioProvide>, Option<FolioConfig>)> {
    let mut expanded = Vec::new();
    let mut all_provides = Vec::new();
    let mut folio_config: Option<FolioConfig> = None;

    for target in targets {
        if let Some(folio_name) = target.strip_prefix('@') {
            let folio_path = find_folio_manifest(folios_dirs, folio_name).ok_or_else(|| {
                folio_err(format!(
                    "folio '{}' not found in {}",
                    folio_name,
                    folios_dirs
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            let manifest = read_manifest(&folio_path)
                .map_err(|e| folio_err(format!("read folio {}: {}", folio_path.display(), e)))?;
            expanded.extend(manifest.folio.plans);
            all_provides.extend(manifest.provides);
            if manifest.config.is_some() {
                folio_config = manifest.config;
            }
        } else {
            expanded.push(target);
        }
    }

    Ok((expanded, all_provides, folio_config))
}

fn folio_err(msg: String) -> WrightError {
    WrightError::PartError(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let m = parse_manifest(
            r#"
[folio]
name = "core"
version = "1"
description = "core"
arch = "x86_64"

plans = ["glibc", "bash"]
"#,
        )
        .unwrap();
        assert_eq!(m.folio.name, "core");
        assert_eq!(m.folio.plans, vec!["glibc", "bash"]);
    }

    #[test]
    fn parses_full_manifest() {
        let m = parse_manifest(
            r#"
[folio]
name = "base"
version = "2026.05"
description = "Base system"
arch = "x86_64"

plans = ["glibc", "bash", "coreutils"]

[[provide]]
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
        assert_eq!(m.folio.name, "base");
        assert_eq!(m.folio.plans.len(), 3);
        assert_eq!(m.provides.len(), 1);
        let cfg = m.config.unwrap();
        assert_eq!(cfg.hostname.as_deref(), Some("wright"));
        assert_eq!(cfg.services, vec!["sshd".to_string()]);
    }
}
