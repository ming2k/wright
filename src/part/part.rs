use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};
use crate::plan::manifest::PlanManifest;

/// Metadata extracted from a .PARTINFO file.
///
/// `.PARTINFO` intentionally carries install-time/runtime metadata only.
/// Link-only rebuild edges remain in plan metadata and are not serialized into
/// binary part metadata.
#[derive(Debug, Clone)]
pub struct PartInfo {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub description: String,
    pub arch: String,
    pub license: String,
    pub install_size: u64,
    pub build_date: String,
    pub runtime_deps: Vec<String>,
    pub replaces: Vec<String>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub backup_files: Vec<String>,
    /// Plan-level metadata (all outputs of a plan share these)
    pub plan_name: String,
    pub plan_version: String,
    pub plan_release: u32,
    pub plan_epoch: u32,
    pub build_deps: Vec<String>,
    pub link_deps: Vec<String>,
}

/// Files that should never be included in a part archive.
/// These are shared/generated files that cause conflicts between parts.
const PART_EXCLUDE_FILES: &[&str] = &["usr/share/info/dir"];

/// Remove well-known files that should never be packaged.
fn purge_excluded_files(part_dir: &Path) {
    for rel in PART_EXCLUDE_FILES {
        let path = part_dir.join(rel);
        if path.exists() {
            tracing::debug!("Removing excluded file from part archive: {}", rel);
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Create a .wright.tar.zst binary part archive.
pub fn create_part(
    part_dir: &Path,
    manifest: &PlanManifest,
    output_path: &Path,
    source_plan: Option<&PlanManifest>,
) -> Result<PathBuf> {
    purge_excluded_files(part_dir);

    // Calculate install size
    let install_size = calculate_dir_size(part_dir)?;

    // Generate .PARTINFO
    let partinfo = generate_partinfo(manifest, install_size, source_plan);

    // Generate .FILELIST
    let filelist = generate_filelist(part_dir)?;

    // Write metadata files into part_dir
    std::fs::write(part_dir.join(".PARTINFO"), &partinfo)
        .map_err(|e| WrightError::PartError(format!("failed to write .PARTINFO: {}", e)))?;

    std::fs::write(part_dir.join(".FILELIST"), &filelist)
        .map_err(|e| WrightError::PartError(format!("failed to write .FILELIST: {}", e)))?;

    // Write .HOOKS (TOML) if install scripts exist
    if let Some(ref scripts) = manifest.install_scripts {
        let hooks_content = generate_hooks_toml(scripts);
        if !hooks_content.is_empty() {
            std::fs::write(part_dir.join(".HOOKS"), &hooks_content)
                .map_err(|e| WrightError::PartError(format!("failed to write .HOOKS: {}", e)))?;
        }
    }

    // Create the archive
    let archive_name = manifest.part_filename();
    let part_path = output_path.join(&archive_name);

    crate::util::compress::create_tar_zst(part_dir, &part_path)?;

    // Clean up metadata files from part_dir
    let _ = std::fs::remove_file(part_dir.join(".PARTINFO"));
    let _ = std::fs::remove_file(part_dir.join(".FILELIST"));
    let _ = std::fs::remove_file(part_dir.join(".HOOKS"));

    Ok(part_path)
}

/// Extract a .wright.tar.zst archive and return the parsed PARTINFO along with
/// the SHA-256 hash of the archive file, computed in a single streaming pass.
pub fn extract_part(part_path: &Path, dest_dir: &Path) -> Result<(PartInfo, String)> {
    let hash = crate::util::compress::extract_tar_zst_hashed(part_path, dest_dir)?;
    let partinfo_path = dest_dir.join(".PARTINFO");
    if partinfo_path.exists() {
        return Ok((parse_partinfo(&partinfo_path)?, hash));
    }

    Err(WrightError::PartError(
        "archive does not contain .PARTINFO".to_string(),
    ))
}

/// Read .PARTINFO from an archive without full extraction.
pub fn read_partinfo(part_path: &Path) -> Result<PartInfo> {
    let file = std::fs::File::open(part_path).map_err(|e| {
        WrightError::PartError(format!("failed to open {}: {}", part_path.display(), e))
    })?;

    let decoder = zstd::Decoder::new(file)
        .map_err(|e| WrightError::PartError(format!("zstd decoder init failed: {}", e)))?;

    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| WrightError::PartError(format!("failed to read archive entries: {}", e)))?
    {
        let mut entry =
            entry.map_err(|e| WrightError::PartError(format!("failed to read entry: {}", e)))?;

        let path = entry
            .path()
            .map_err(|e| WrightError::PartError(format!("failed to read entry path: {}", e)))?;

        let path_str = path.to_string_lossy();
        if path_str.ends_with(".PARTINFO") {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::PartError(format!("failed to read .PARTINFO: {}", e)))?;
            return parse_partinfo_str(&content);
        }
    }

    Err(WrightError::PartError(
        "archive does not contain .PARTINFO".to_string(),
    ))
}

fn generate_partinfo(
    manifest: &PlanManifest,
    install_size: u64,
    source_plan: Option<&PlanManifest>,
) -> String {
    let build_date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Determine plan-level metadata: either from the original plan manifest
    // or from the current manifest itself (single-output plans).
    let plan = source_plan.unwrap_or(manifest);
    let plan_name = &plan.plan.name;
    let plan_version = plan.plan.version.as_deref().unwrap_or("");
    let _plan_release = plan.plan.release;
    let plan_epoch = plan.plan.epoch;

    let mut deps_toml = String::new();
    if !manifest.runtime_deps.is_empty() {
        deps_toml.push_str("\n[dependencies]\n");
        deps_toml.push_str("runtime = [");
        for (i, dep) in manifest.runtime_deps.iter().enumerate() {
            if i > 0 {
                deps_toml.push_str(", ");
            }
            deps_toml.push_str(&format!("\"{}\"", dep));
        }
        deps_toml.push_str("]\n");
    }

    let mut relations_toml = String::new();
    if !manifest.relations.replaces.is_empty()
        || !manifest.relations.conflicts.is_empty()
        || !manifest.relations.provides.is_empty()
    {
        relations_toml.push_str("\n[relations]\n");
        if !manifest.relations.replaces.is_empty() {
            relations_toml.push_str("replaces = [");
            for (i, dep) in manifest.relations.replaces.iter().enumerate() {
                if i > 0 {
                    relations_toml.push_str(", ");
                }
                relations_toml.push_str(&format!("\"{}\"", dep));
            }
            relations_toml.push_str("]\n");
        }
        if !manifest.relations.conflicts.is_empty() {
            relations_toml.push_str("conflicts = [");
            for (i, dep) in manifest.relations.conflicts.iter().enumerate() {
                if i > 0 {
                    relations_toml.push_str(", ");
                }
                relations_toml.push_str(&format!("\"{}\"", dep));
            }
            relations_toml.push_str("]\n");
        }
        if !manifest.relations.provides.is_empty() {
            relations_toml.push_str("provides = [");
            for (i, dep) in manifest.relations.provides.iter().enumerate() {
                if i > 0 {
                    relations_toml.push_str(", ");
                }
                relations_toml.push_str(&format!("\"{}\"", dep));
            }
            relations_toml.push_str("]\n");
        }
    }

    let mut backup_toml = String::new();
    if let Some(ref backup) = manifest.backup {
        if !backup.files.is_empty() {
            backup_toml.push_str("\n[backup]\nfiles = [");
            for (i, f) in backup.files.iter().enumerate() {
                if i > 0 {
                    backup_toml.push_str(", ");
                }
                backup_toml.push_str(&format!("\"{}\"", f));
            }
            backup_toml.push_str("]\n");
        }
    }

    let epoch_line = if manifest.plan.epoch > 0 {
        format!("epoch = {}\n", manifest.plan.epoch)
    } else {
        String::new()
    };

    let version_line = match manifest.plan.version.as_deref() {
        Some(v) if !v.is_empty() => format!("version = \"{}\"\n", v),
        _ => String::new(),
    };

    let plan_epoch_line = if plan_epoch > 0 {
        format!("plan_epoch = {}\n", plan_epoch)
    } else {
        String::new()
    };

    let plan_version_line = if !plan_version.is_empty() {
        format!("plan_version = \"{}\"\n", plan_version)
    } else {
        String::new()
    };

    let mut plan_deps_toml = String::new();
    if !plan.build_deps.is_empty() || !plan.link_deps.is_empty() {
        plan_deps_toml.push_str("\n[plan]\n");
        if !plan.build_deps.is_empty() {
            plan_deps_toml.push_str("build_deps = [");
            for (i, d) in plan.build_deps.iter().enumerate() {
                if i > 0 {
                    plan_deps_toml.push_str(", ");
                }
                plan_deps_toml.push_str(&format!("\"{}\"", d));
            }
            plan_deps_toml.push_str("]\n");
        }
        if !plan.link_deps.is_empty() {
            plan_deps_toml.push_str("link_deps = [");
            for (i, d) in plan.link_deps.iter().enumerate() {
                if i > 0 {
                    plan_deps_toml.push_str(", ");
                }
                plan_deps_toml.push_str(&format!("\"{}\"", d));
            }
            plan_deps_toml.push_str("]\n");
        }
    }

    format!(
        r#"[part]
name = "{name}"
plan_name = "{plan_name}"
{version}release = {release}
{epoch}description = "{description}"
arch = "{arch}"
license = "{license}"
install_size = {install_size}
build_date = "{build_date}"
packager = "wright {wright_version}"
{plan_version}{plan_epoch}{deps}{relations}{backup}{plan_deps}
"#,
        name = manifest.plan.name,
        plan_name = plan_name,
        version = version_line,
        release = manifest.plan.release,
        epoch = epoch_line,
        description = manifest.plan.description,
        arch = manifest.plan.arch,
        license = manifest.plan.license,
        install_size = install_size,
        build_date = build_date,
        wright_version = env!("CARGO_PKG_VERSION"),
        plan_version = plan_version_line,
        plan_epoch = plan_epoch_line,
        deps = deps_toml,
        relations = relations_toml,
        backup = backup_toml,
        plan_deps = plan_deps_toml,
    )
}

fn generate_filelist(part_dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    for entry in WalkDir::new(part_dir).sort_by_file_name() {
        let entry = entry
            .map_err(|e| WrightError::PartError(format!("failed to walk directory: {}", e)))?;
        let relative = entry.path().strip_prefix(part_dir).unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy();
        // Skip metadata files and root
        if relative_str.is_empty()
            || relative_str.starts_with(".PARTINFO")
            || relative_str.starts_with(".FILELIST")
            || relative_str.starts_with(".HOOKS")
        {
            continue;
        }
        files.push(format!("/{}", relative_str));
    }
    Ok(files.join("\n"))
}

/// Generate `.HOOKS` content in TOML format.
///
/// ```toml
/// [hooks]
/// post_install = "ldconfig"
/// post_upgrade = "systemctl reload nginx"
/// pre_remove = "systemctl stop nginx"
/// post_remove = "userdel nginx"
/// ```
fn generate_hooks_toml(scripts: &crate::plan::manifest::InstallScripts) -> String {
    let has_any = scripts.pre_install.is_some()
        || scripts.post_install.is_some()
        || scripts.post_upgrade.is_some()
        || scripts.pre_remove.is_some()
        || scripts.post_remove.is_some();
    if !has_any {
        return String::new();
    }

    let mut content = String::from("[hooks]\n");
    for (key, value) in [
        ("pre_install", &scripts.pre_install),
        ("post_install", &scripts.post_install),
        ("post_upgrade", &scripts.post_upgrade),
        ("pre_remove", &scripts.pre_remove),
        ("post_remove", &scripts.post_remove),
    ] {
        if let Some(ref s) = value {
            let trimmed = s.trim();
            if trimmed.contains('\n') {
                content.push_str(&format!("{} = \"\"\"\n{}\n\"\"\"\n", key, trimmed));
            } else {
                content.push_str(&format!(
                    "{} = \"{}\"\n",
                    key,
                    trimmed.replace('\\', "\\\\").replace('"', "\\\"")
                ));
            }
        }
    }
    content
}

fn calculate_dir_size(dir: &Path) -> Result<u64> {
    let mut size = 0;
    for entry in WalkDir::new(dir) {
        let entry = entry
            .map_err(|e| WrightError::PartError(format!("failed to walk directory: {}", e)))?;
        if entry.file_type().is_file() {
            size += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(size)
}

fn parse_partinfo(path: &Path) -> Result<PartInfo> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| WrightError::PartError(format!("failed to read .PARTINFO: {}", e)))?;
    parse_partinfo_str(&content)
}

fn parse_partinfo_str(content: &str) -> Result<PartInfo> {
    #[derive(serde::Deserialize)]
    struct PartInfoToml {
        part: PartInfoMeta,
        #[serde(default)]
        plan: Option<PartInfoPlan>,
        #[serde(default)]
        dependencies: Option<PartInfoDeps>,
        #[serde(default)]
        relations: Option<PartInfoRelations>,
        #[serde(default)]
        backup: Option<PartInfoBackup>,
    }

    #[derive(serde::Deserialize)]
    struct PartInfoMeta {
        name: String,
        plan_name: String,
        #[serde(default)]
        plan_version: String,
        #[serde(default)]
        plan_release: u32,
        #[serde(default)]
        plan_epoch: u32,
        #[serde(default)]
        version: String,
        release: u32,
        #[serde(default)]
        epoch: u32,
        description: String,
        arch: String,
        license: String,
        #[serde(default)]
        install_size: u64,
        #[serde(default)]
        build_date: String,
    }

    #[derive(serde::Deserialize, Default)]
    struct PartInfoPlan {
        #[serde(default)]
        build_deps: Vec<String>,
        #[serde(default)]
        link_deps: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PartInfoDeps {
        #[serde(default)]
        runtime: Vec<String>,
    }

    #[derive(serde::Deserialize, Default)]
    struct PartInfoRelations {
        #[serde(default)]
        replaces: Vec<String>,
        #[serde(default)]
        conflicts: Vec<String>,
        #[serde(default)]
        provides: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    struct PartInfoBackup {
        #[serde(default)]
        files: Vec<String>,
    }

    let parsed: PartInfoToml = toml::from_str(content)
        .map_err(|e| WrightError::PartError(format!("failed to parse .PARTINFO: {}", e)))?;

    let runtime_deps = parsed
        .dependencies
        .map(|d| d.runtime)
        .unwrap_or_default();

    let relations = parsed.relations.unwrap_or_default();
    let plan_section = parsed.plan.unwrap_or_default();

    Ok(PartInfo {
        name: parsed.part.name,
        version: parsed.part.version,
        release: parsed.part.release,
        epoch: parsed.part.epoch,
        description: parsed.part.description,
        arch: parsed.part.arch,
        license: parsed.part.license,
        install_size: parsed.part.install_size,
        build_date: parsed.part.build_date,
        runtime_deps,
        replaces: relations.replaces,
        conflicts: relations.conflicts,
        provides: relations.provides,
        backup_files: parsed.backup.map(|b| b.files).unwrap_or_default(),
        plan_name: parsed.part.plan_name,
        plan_version: parsed.part.plan_version,
        plan_release: parsed.part.plan_release,
        plan_epoch: parsed.part.plan_epoch,
        build_deps: plan_section.build_deps,
        link_deps: plan_section.link_deps,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_partinfo_str;

    #[test]
    fn parse_partinfo_accepts_runtime_dependencies() {
        let info = parse_partinfo_str(
            r#"
[part]
name = "demo"
plan_name = "demo"
version = "1.0.0"
release = 1
description = "demo"
arch = "x86_64"
license = "MIT"

[dependencies]
runtime = ["bash"]
"#,
        )
        .unwrap();

        assert_eq!(info.runtime_deps, vec!["bash"]);
    }
}
