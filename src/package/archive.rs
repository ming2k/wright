use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;

use crate::error::{WrightError, Result};
use crate::package::manifest::PackageManifest;

/// Metadata extracted from a .PKGINFO file
#[derive(Debug, Clone)]
pub struct PkgInfo {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub description: String,
    pub arch: String,
    pub license: String,
    pub install_size: u64,
    pub build_date: String,
    pub runtime_deps: Vec<String>,
    pub backup_files: Vec<String>,
}

/// Create a .wright.tar.zst binary package archive.
pub fn create_archive(
    pkg_dir: &Path,
    manifest: &PackageManifest,
    output_path: &Path,
) -> Result<PathBuf> {
    // Calculate install size
    let install_size = calculate_dir_size(pkg_dir)?;

    // Generate .PKGINFO
    let pkginfo = generate_pkginfo(manifest, install_size);

    // Generate .FILELIST
    let filelist = generate_filelist(pkg_dir)?;

    // Write metadata files into pkg_dir
    std::fs::write(pkg_dir.join(".PKGINFO"), &pkginfo).map_err(|e| {
        WrightError::ArchiveError(format!("failed to write .PKGINFO: {}", e))
    })?;

    std::fs::write(pkg_dir.join(".FILELIST"), &filelist).map_err(|e| {
        WrightError::ArchiveError(format!("failed to write .FILELIST: {}", e))
    })?;

    // Write .INSTALL if install scripts exist
    if let Some(ref scripts) = manifest.install_scripts {
        let install_content = generate_install_scripts(scripts);
        if !install_content.is_empty() {
            std::fs::write(pkg_dir.join(".INSTALL"), &install_content).map_err(|e| {
                WrightError::ArchiveError(format!("failed to write .INSTALL: {}", e))
            })?;
        }
    }

    // Create the archive
    let archive_name = manifest.archive_filename();
    let archive_path = output_path.join(&archive_name);

    crate::util::compress::create_tar_zst(pkg_dir, &archive_path)?;

    // Clean up metadata files from pkg_dir
    let _ = std::fs::remove_file(pkg_dir.join(".PKGINFO"));
    let _ = std::fs::remove_file(pkg_dir.join(".FILELIST"));
    let _ = std::fs::remove_file(pkg_dir.join(".INSTALL"));

    Ok(archive_path)
}

/// Extract a .wright.tar.zst archive and return the parsed PKGINFO.
pub fn extract_archive(archive_path: &Path, dest_dir: &Path) -> Result<PkgInfo> {
    crate::util::compress::extract_tar_zst(archive_path, dest_dir)?;
    let pkginfo_path = dest_dir.join(".PKGINFO");
    parse_pkginfo(&pkginfo_path)
}

/// Read .PKGINFO from an archive without full extraction.
pub fn read_pkginfo(archive_path: &Path) -> Result<PkgInfo> {
    let file = std::fs::File::open(archive_path).map_err(|e| {
        WrightError::ArchiveError(format!("failed to open {}: {}", archive_path.display(), e))
    })?;

    let decoder = zstd::Decoder::new(file).map_err(|e| {
        WrightError::ArchiveError(format!("zstd decoder init failed: {}", e))
    })?;

    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().map_err(|e| {
        WrightError::ArchiveError(format!("failed to read archive entries: {}", e))
    })? {
        let mut entry = entry.map_err(|e| {
            WrightError::ArchiveError(format!("failed to read entry: {}", e))
        })?;

        let path = entry.path().map_err(|e| {
            WrightError::ArchiveError(format!("failed to read entry path: {}", e))
        })?;

        if path.to_string_lossy().ends_with(".PKGINFO") {
            let mut content = String::new();
            entry.read_to_string(&mut content).map_err(|e| {
                WrightError::ArchiveError(format!("failed to read .PKGINFO: {}", e))
            })?;
            return parse_pkginfo_str(&content);
        }
    }

    Err(WrightError::ArchiveError(
        "archive does not contain .PKGINFO".to_string(),
    ))
}

fn generate_pkginfo(manifest: &PackageManifest, install_size: u64) -> String {
    let build_date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut deps_toml = String::new();
    if !manifest.dependencies.runtime.is_empty() {
        deps_toml.push_str("\n[dependencies]\nruntime = [");
        for (i, dep) in manifest.dependencies.runtime.iter().enumerate() {
            if i > 0 {
                deps_toml.push_str(", ");
            }
            deps_toml.push_str(&format!("\"{}\"", dep));
        }
        deps_toml.push(']');
    }

    let mut backup_toml = String::new();
    if let Some(ref backup) = manifest.backup {
        if !backup.files.is_empty() {
            backup_toml.push_str("\n\n[backup]\nfiles = [");
            for (i, f) in backup.files.iter().enumerate() {
                if i > 0 {
                    backup_toml.push_str(", ");
                }
                backup_toml.push_str(&format!("\"{}\"", f));
            }
            backup_toml.push(']');
        }
    }

    format!(
        r#"[package]
name = "{name}"
version = "{version}"
release = {release}
description = "{description}"
arch = "{arch}"
license = "{license}"
install_size = {install_size}
build_date = "{build_date}"
packager = "wright-build {wright_version}"
{deps}{backup}
"#,
        name = manifest.package.name,
        version = manifest.package.version,
        release = manifest.package.release,
        description = manifest.package.description,
        arch = manifest.package.arch,
        license = manifest.package.license,
        install_size = install_size,
        build_date = build_date,
        wright_version = env!("CARGO_PKG_VERSION"),
        deps = deps_toml,
        backup = backup_toml,
    )
}

fn generate_filelist(pkg_dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    for entry in WalkDir::new(pkg_dir).sort_by_file_name() {
        let entry = entry.map_err(|e| {
            WrightError::ArchiveError(format!("failed to walk directory: {}", e))
        })?;
        let relative = entry.path().strip_prefix(pkg_dir).unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy();
        // Skip metadata files and root
        if relative_str.is_empty()
            || relative_str.starts_with(".PKGINFO")
            || relative_str.starts_with(".FILELIST")
            || relative_str.starts_with(".INSTALL")
        {
            continue;
        }
        files.push(format!("/{}", relative_str));
    }
    Ok(files.join("\n"))
}

fn generate_install_scripts(
    scripts: &crate::package::manifest::InstallScripts,
) -> String {
    let mut content = String::new();
    if let Some(ref s) = scripts.post_install {
        content.push_str("[post_install]\n");
        content.push_str(s);
        content.push('\n');
    }
    if let Some(ref s) = scripts.post_upgrade {
        content.push_str("[post_upgrade]\n");
        content.push_str(s);
        content.push('\n');
    }
    if let Some(ref s) = scripts.pre_remove {
        content.push_str("[pre_remove]\n");
        content.push_str(s);
        content.push('\n');
    }
    content
}

fn calculate_dir_size(dir: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in WalkDir::new(dir) {
        let entry = entry.map_err(|e| {
            WrightError::ArchiveError(format!("failed to walk directory: {}", e))
        })?;
        if entry.file_type().is_file() {
            size += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(size)
}

fn parse_pkginfo(path: &Path) -> Result<PkgInfo> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        WrightError::ArchiveError(format!("failed to read .PKGINFO: {}", e))
    })?;
    parse_pkginfo_str(&content)
}

fn parse_pkginfo_str(content: &str) -> Result<PkgInfo> {
    #[derive(serde::Deserialize)]
    struct PkgInfoToml {
        package: PkgInfoPackage,
        #[serde(default)]
        dependencies: Option<PkgInfoDeps>,
        #[serde(default)]
        backup: Option<PkgInfoBackup>,
    }

    #[derive(serde::Deserialize)]
    struct PkgInfoPackage {
        name: String,
        version: String,
        release: u32,
        description: String,
        arch: String,
        license: String,
        #[serde(default)]
        install_size: u64,
        #[serde(default)]
        build_date: String,
    }

    #[derive(serde::Deserialize)]
    struct PkgInfoDeps {
        #[serde(default)]
        runtime: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    struct PkgInfoBackup {
        #[serde(default)]
        files: Vec<String>,
    }

    let parsed: PkgInfoToml = toml::from_str(content).map_err(|e| {
        WrightError::ArchiveError(format!("failed to parse .PKGINFO: {}", e))
    })?;

    Ok(PkgInfo {
        name: parsed.package.name,
        version: parsed.package.version,
        release: parsed.package.release,
        description: parsed.package.description,
        arch: parsed.package.arch,
        license: parsed.package.license,
        install_size: parsed.package.install_size,
        build_date: parsed.package.build_date,
        runtime_deps: parsed
            .dependencies
            .map(|d| d.runtime)
            .unwrap_or_default(),
        backup_files: parsed.backup.map(|b| b.files).unwrap_or_default(),
    })
}
