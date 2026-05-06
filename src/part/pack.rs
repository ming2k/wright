//! Pack format: a `.wright.pack.tar` artifact bundling a manifest, the part
//! archives it references, and an optional overlay tree applied after install.
//!
//! See `docs/adr/0014-launch-and-pack-format.md` for design and rationale.
//!
//! On-disk layout (uncompressed tar; parts inside are already zstd-compressed):
//!
//! ```text
//! <pack>.wright.pack.tar
//! ├── pack.toml
//! ├── parts/<name>-<ver>-<rel>-<arch>.wright.tar.zst
//! └── overlay/...                (optional)
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, WrightError};

pub const PACK_MANIFEST_NAME: &str = "pack.toml";
pub const PACK_PARTS_DIR: &str = "parts";
pub const PACK_OVERLAY_DIR: &str = "overlay";
pub const PACK_FILE_SUFFIX: &str = ".wright.pack.tar";

/// Top-level pack manifest. Loaded from `pack.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub pack: PackMeta,

    #[serde(default, rename = "part")]
    pub parts: Vec<PackPart>,

    #[serde(default, rename = "assume")]
    pub assumes: Vec<PackAssume>,

    #[serde(default)]
    pub config: Option<PackConfig>,

    /// SHA-256 of the overlay tar payload, when an overlay was bundled.
    /// `wright pack` writes it; `wright launch` verifies it.
    #[serde(default)]
    pub overlay_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackPart {
    /// Path inside the pack tar, e.g. `parts/glibc-2.41-1-x86_64.wright.tar.zst`.
    pub file: String,
    #[serde(default)]
    pub origin: PackOrigin,
    /// SHA-256 of the part archive bytes. Filled in by `wright pack`.
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackOrigin {
    Manual,
    Dependency,
}

impl Default for PackOrigin {
    fn default() -> Self {
        PackOrigin::Manual
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackAssume {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackConfig {
    #[serde(default)]
    pub hostname: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    /// runit service names; launch creates `/var/service/<name>` symlinks pointing
    /// at `/etc/sv/<name>` after the overlay is applied.
    #[serde(default)]
    pub services: Vec<String>,
}

/// Result of opening a pack: the parsed manifest and the path of the source tar.
pub struct OpenedPack {
    pub manifest: PackManifest,
    pub source: PathBuf,
}

/// Read just the manifest from a pack archive without extracting anything.
pub fn read_manifest(pack_path: &Path) -> Result<PackManifest> {
    let file = File::open(pack_path)
        .map_err(|e| pack_err(format!("failed to open {}: {}", pack_path.display(), e)))?;
    let mut archive = tar::Archive::new(file);
    for entry in archive
        .entries()
        .map_err(|e| pack_err(format!("failed to read pack entries: {}", e)))?
    {
        let mut entry = entry.map_err(|e| pack_err(format!("failed to read pack entry: {}", e)))?;
        let path = entry
            .path()
            .map_err(|e| pack_err(format!("failed to read entry path: {}", e)))?;
        if path.as_os_str() == PACK_MANIFEST_NAME {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| pack_err(format!("failed to read pack.toml: {}", e)))?;
            return parse_manifest(&content);
        }
    }
    Err(pack_err("pack archive does not contain pack.toml".into()))
}

/// Parse `pack.toml` content. Public for tests and for `wright pack` validation.
pub fn parse_manifest(content: &str) -> Result<PackManifest> {
    toml::from_str(content).map_err(|e| pack_err(format!("invalid pack.toml: {}", e)))
}

/// Extract a pack archive into `dest_dir`. Refuses paths that escape the dest.
pub fn extract_pack(pack_path: &Path, dest_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        pack_err(format!(
            "failed to create pack extract dir {}: {}",
            dest_dir.display(),
            e
        ))
    })?;

    let file = File::open(pack_path)
        .map_err(|e| pack_err(format!("failed to open {}: {}", pack_path.display(), e)))?;
    let mut archive = tar::Archive::new(file);
    archive.set_overwrite(true);
    for entry in archive
        .entries()
        .map_err(|e| pack_err(format!("failed to read pack entries: {}", e)))?
    {
        let mut entry = entry.map_err(|e| pack_err(format!("failed to read pack entry: {}", e)))?;
        let path = entry
            .path()
            .map_err(|e| pack_err(format!("failed to read entry path: {}", e)))?;
        if !is_safe_pack_path(&path) {
            return Err(pack_err(format!("unsafe path in pack: {}", path.display())));
        }
        entry
            .unpack_in(dest_dir)
            .map_err(|e| pack_err(format!("failed to extract pack entry: {}", e)))?;
    }
    Ok(())
}

/// Build a pack from `source_dir`, which must contain `pack.toml` and the part
/// archives it references. Computes SHA-256 hashes for every part and the
/// overlay tar (if present) and writes them back into the manifest before
/// archiving. Returns the path of the produced pack file.
pub fn create_pack(source_dir: &Path, output_path: &Path) -> Result<PathBuf> {
    let manifest_path = source_dir.join(PACK_MANIFEST_NAME);
    if !manifest_path.is_file() {
        return Err(pack_err(format!(
            "{} is missing in {}",
            PACK_MANIFEST_NAME,
            source_dir.display()
        )));
    }
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| pack_err(format!("failed to read pack.toml: {}", e)))?;
    let mut manifest = parse_manifest(&raw)?;

    // Verify and hash every referenced part.
    for part in &mut manifest.parts {
        let part_path = source_dir.join(&part.file);
        if !part_path.is_file() {
            return Err(pack_err(format!(
                "part archive {} declared in manifest is missing",
                part.file
            )));
        }
        let hash = crate::util::checksum::sha256_file(&part_path)?;
        part.sha256 = Some(hash);
    }

    // Optional overlay: package as a single tar so it can be hashed once, applied
    // atomically into the target during launch, and verified later. We keep it
    // uncompressed since the rest of the pack is uncompressed too.
    let overlay_dir = source_dir.join(PACK_OVERLAY_DIR);
    let overlay_tar_path = if overlay_dir.is_dir() {
        let path = source_dir.join(format!("{}.tar", PACK_OVERLAY_DIR));
        write_directory_as_tar(&overlay_dir, &path)?;
        manifest.overlay_sha256 = Some(crate::util::checksum::sha256_file(&path)?);
        Some(path)
    } else {
        manifest.overlay_sha256 = None;
        None
    };

    let manifest_bytes =
        toml::to_string_pretty(&manifest).map_err(|e| pack_err(format!("serialize: {}", e)))?;

    let out = File::create(output_path).map_err(|e| {
        pack_err(format!(
            "failed to create pack {}: {}",
            output_path.display(),
            e
        ))
    })?;
    let mut builder = tar::Builder::new(out);
    builder.follow_symlinks(false);

    append_bytes(&mut builder, PACK_MANIFEST_NAME, manifest_bytes.as_bytes())?;

    for part in &manifest.parts {
        let part_path = source_dir.join(&part.file);
        let dest = normalize_pack_entry(&part.file)?;
        append_file(&mut builder, &dest, &part_path)?;
    }

    if let Some(ref overlay_tar) = overlay_tar_path {
        append_file(&mut builder, Path::new("overlay.tar"), overlay_tar)?;
    }

    builder
        .finish()
        .map_err(|e| pack_err(format!("failed to finalize pack: {}", e)))?;

    if let Some(ref overlay_tar) = overlay_tar_path {
        let _ = std::fs::remove_file(overlay_tar);
    }

    Ok(output_path.to_path_buf())
}

/// Default output filename when `wright pack` is invoked without `-o`.
pub fn default_output_filename(manifest: &PackManifest) -> String {
    format!(
        "{}-{}{}",
        manifest.pack.name, manifest.pack.version, PACK_FILE_SUFFIX
    )
}

/// Verify that every part archive inside an extracted pack matches the SHA-256
/// recorded in the manifest. Returns the list of mismatches (empty == clean).
pub fn verify_extracted_pack(manifest: &PackManifest, extract_dir: &Path) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    for part in &manifest.parts {
        let path = extract_dir.join(&part.file);
        let actual = crate::util::checksum::sha256_file(&path)?;
        match part.sha256.as_deref() {
            Some(expected) if expected == actual => {}
            Some(expected) => mismatches.push(format!(
                "{}: expected {}, got {}",
                part.file, expected, actual
            )),
            None => mismatches.push(format!("{}: manifest has no sha256 entry", part.file)),
        }
    }
    if let Some(expected) = manifest.overlay_sha256.as_deref() {
        let overlay_tar = extract_dir.join("overlay.tar");
        if overlay_tar.is_file() {
            let actual = crate::util::checksum::sha256_file(&overlay_tar)?;
            if actual != expected {
                mismatches.push(format!(
                    "overlay.tar: expected {}, got {}",
                    expected, actual
                ));
            }
        } else {
            mismatches.push("overlay.tar: missing from pack".to_string());
        }
    }
    Ok(mismatches)
}

/// Apply an overlay tar (as written by `create_pack`) to a target root, skipping
/// any path whose owning part has already installed it. Returns the list of
/// paths that were actually written.
pub fn apply_overlay_tar(
    overlay_tar: &Path,
    root_dir: &Path,
    skip_paths: &std::collections::HashSet<String>,
) -> Result<Vec<String>> {
    let file =
        File::open(overlay_tar).map_err(|e| pack_err(format!("failed to open overlay: {}", e)))?;
    let mut archive = tar::Archive::new(file);
    archive.set_overwrite(false);
    let mut written = Vec::new();
    for entry in archive
        .entries()
        .map_err(|e| pack_err(format!("failed to read overlay: {}", e)))?
    {
        let mut entry =
            entry.map_err(|e| pack_err(format!("failed to read overlay entry: {}", e)))?;
        let entry_path = entry
            .path()
            .map_err(|e| pack_err(format!("failed to read overlay entry path: {}", e)))?
            .into_owned();
        if !is_safe_pack_path(&entry_path) {
            return Err(pack_err(format!(
                "unsafe overlay path: {}",
                entry_path.display()
            )));
        }
        let absolute = format!("/{}", entry_path.to_string_lossy());
        if skip_paths.contains(&absolute) {
            continue;
        }
        // Skip entries that already exist on the target unless they are
        // directories (which we always merge).
        let dest = root_dir.join(&entry_path);
        if dest.exists() && !dest.is_dir() {
            continue;
        }
        entry
            .unpack_in(root_dir)
            .map_err(|e| pack_err(format!("failed to extract overlay entry: {}", e)))?;
        written.push(absolute);
    }
    Ok(written)
}

fn write_directory_as_tar(source_dir: &Path, output_path: &Path) -> Result<()> {
    let out = File::create(output_path)
        .map_err(|e| pack_err(format!("failed to create overlay tar: {}", e)))?;
    let mut builder = tar::Builder::new(out);
    builder.follow_symlinks(false);
    for entry in walkdir::WalkDir::new(source_dir)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|e| pack_err(format!("failed to walk overlay: {}", e)))?;
        let rel = entry
            .path()
            .strip_prefix(source_dir)
            .unwrap_or_else(|_| entry.path());
        builder
            .append_path_with_name(entry.path(), rel)
            .map_err(|e| pack_err(format!("failed to add overlay entry: {}", e)))?;
    }
    builder
        .finish()
        .map_err(|e| pack_err(format!("failed to finalize overlay: {}", e)))?;
    Ok(())
}

fn append_bytes<W: Write>(builder: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, name, bytes)
        .map_err(|e| pack_err(format!("failed to append {}: {}", name, e)))
}

fn append_file<W: Write>(
    builder: &mut tar::Builder<W>,
    name_in_archive: &Path,
    src: &Path,
) -> Result<()> {
    builder
        .append_path_with_name(src, name_in_archive)
        .map_err(|e| {
            pack_err(format!(
                "failed to add {}: {}",
                name_in_archive.display(),
                e
            ))
        })
}

fn normalize_pack_entry(rel: &str) -> Result<PathBuf> {
    let path = PathBuf::from(rel);
    if !is_safe_pack_path(&path) {
        return Err(pack_err(format!("unsafe pack entry: {}", rel)));
    }
    Ok(path)
}

fn is_safe_pack_path(path: &Path) -> bool {
    for comp in path.components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
            Component::Normal(_) | Component::CurDir => {}
        }
    }
    true
}

fn pack_err(msg: String) -> WrightError {
    WrightError::PartError(msg)
}

/// Group parts by directory for a quick summary, used by `wright pack inspect`.
pub fn summarize_by_dir(manifest: &PackManifest) -> BTreeMap<String, usize> {
    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    for part in &manifest.parts {
        let bucket = Path::new(&part.file)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());
        *totals.entry(bucket).or_default() += 1;
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let m = parse_manifest(
            r#"
[pack]
name = "demo"
version = "1"
description = "demo"
arch = "x86_64"

[[part]]
file = "parts/foo-1-1-x86_64.wright.tar.zst"
"#,
        )
        .unwrap();
        assert_eq!(m.pack.name, "demo");
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0].origin, PackOrigin::Manual);
    }

    #[test]
    fn parses_assumes_and_config() {
        let m = parse_manifest(
            r#"
[pack]
name = "demo"
version = "1"

[[assume]]
name = "linux"
version = "6.12.0"

[config]
hostname = "wright"
timezone = "UTC"
services = ["sshd"]
"#,
        )
        .unwrap();
        assert_eq!(m.assumes.len(), 1);
        assert_eq!(m.assumes[0].name, "linux");
        let cfg = m.config.unwrap();
        assert_eq!(cfg.hostname.as_deref(), Some("wright"));
        assert_eq!(cfg.services, vec!["sshd".to_string()]);
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(!is_safe_pack_path(Path::new("../etc/passwd")));
        assert!(!is_safe_pack_path(Path::new("/etc/passwd")));
        assert!(is_safe_pack_path(Path::new("parts/foo.wright.tar.zst")));
    }
}
