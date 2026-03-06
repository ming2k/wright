use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{WrightError, Result};
use crate::package::archive;

const INDEX_FILENAME: &str = "wright.index.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoIndex {
    #[serde(default)]
    pub packages: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    pub version: String,
    pub release: u32,
    #[serde(default)]
    pub epoch: u32,
    pub description: String,
    pub arch: String,
    pub filename: String,
    pub sha256: String,
    pub install_size: u64,
    #[serde(default)]
    pub runtime_deps: Vec<String>,
    #[serde(default)]
    pub link_deps: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub replaces: Vec<String>,
}

pub fn index_path(repo_dir: &Path) -> PathBuf {
    repo_dir.join(INDEX_FILENAME)
}

/// Generate an index for all `.wright.tar.zst` packages in `repo_dir`.
pub fn generate_index(repo_dir: &Path) -> Result<RepoIndex> {
    let mut entries = Vec::new();

    if !repo_dir.exists() {
        return Ok(RepoIndex { packages: entries });
    }

    for entry in std::fs::read_dir(repo_dir).map_err(WrightError::IoError)? {
        let entry = entry.map_err(WrightError::IoError)?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.ends_with(".wright.tar.zst") {
            continue;
        }

        let pkginfo = match archive::read_pkginfo(&path) {
            Ok(info) => info,
            Err(e) => {
                tracing::warn!("Skipping {}: {}", path.display(), e);
                continue;
            }
        };

        let sha256 = crate::util::checksum::sha256_file(&path)?;

        entries.push(IndexEntry {
            name: pkginfo.name,
            version: pkginfo.version,
            release: pkginfo.release,
            epoch: pkginfo.epoch,
            description: pkginfo.description,
            arch: pkginfo.arch,
            filename: name.to_string(),
            sha256,
            install_size: pkginfo.install_size,
            runtime_deps: pkginfo.runtime_deps,
            link_deps: pkginfo.link_deps,
            provides: pkginfo.provides,
            conflicts: pkginfo.conflicts,
            replaces: pkginfo.replaces,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(RepoIndex { packages: entries })
}

/// Write an index file to disk.
pub fn write_index(index: &RepoIndex, repo_dir: &Path) -> Result<()> {
    let content = toml::to_string_pretty(index).map_err(|e| {
        WrightError::ConfigError(format!("failed to serialize index: {}", e))
    })?;
    let path = index_path(repo_dir);
    std::fs::write(&path, content).map_err(|e| {
        WrightError::IoError(std::io::Error::new(std::io::ErrorKind::Other,
            format!("failed to write {}: {}", path.display(), e)))
    })?;
    Ok(())
}

/// Read an index file from disk.
pub fn read_index(repo_dir: &Path) -> Result<Option<RepoIndex>> {
    let path = index_path(repo_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        WrightError::ConfigError(format!("failed to read {}: {}", path.display(), e))
    })?;
    let index: RepoIndex = toml::from_str(&content).map_err(|e| {
        WrightError::ConfigError(format!("failed to parse {}: {}", path.display(), e))
    })?;
    Ok(Some(index))
}
