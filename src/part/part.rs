use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};
use crate::plan::manifest::PlanManifest;

/// Plan-level metadata extracted from the `[plan]` section of `.PARTINFO`.
/// All outputs of a plan share these fields; they are stored in the `plans` table.
#[derive(Debug, Clone)]
pub struct PlanMetadata {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub description: String,
    pub arch: String,
    pub license: String,
    pub build_deps: Vec<String>,
    pub link_deps: Vec<String>,
}

/// Metadata extracted from a .PARTINFO file.
///
/// `.PARTINFO` intentionally carries install-time/runtime metadata only.
/// Link-only rebuild edges remain in plan metadata and are not serialized into
/// binary part metadata.
#[derive(Debug, Clone)]
pub struct PartInfo {
    pub name: String,
    pub build_date: String,
    pub runtime_deps: Vec<String>,
    pub replaces: Vec<String>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub backup_files: Vec<String>,
    pub plan: PlanMetadata,
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

    // Generate .PARTINFO
    let partinfo = generate_partinfo(manifest, source_plan);

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

fn generate_partinfo(manifest: &PlanManifest, source_plan: Option<&PlanManifest>) -> String {
    let build_date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Determine plan-level metadata: either from the original plan manifest
    // or from the current manifest itself (single-output plans).
    let plan = source_plan.unwrap_or(manifest);

    let mut runtime_deps_toml = String::new();
    if !manifest.runtime_deps.is_empty() {
        runtime_deps_toml.push_str("runtime_deps = [");
        for (i, dep) in manifest.runtime_deps.iter().enumerate() {
            if i > 0 {
                runtime_deps_toml.push_str(", ");
            }
            runtime_deps_toml.push_str(&format!("\"{}\"", dep));
        }
        runtime_deps_toml.push_str("]\n");
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

    let mut plan_toml = String::new();
    plan_toml.push_str("\n[plan]\n");
    plan_toml.push_str(&format!("name = \"{}\"\n", plan.metadata.name));
    if let Some(ref v) = plan.metadata.version {
        if !v.is_empty() {
            plan_toml.push_str(&format!("version = \"{}\"\n", v));
        }
    }
    plan_toml.push_str(&format!("release = {}\n", plan.metadata.release));
    if plan.metadata.epoch > 0 {
        plan_toml.push_str(&format!("epoch = {}\n", plan.metadata.epoch));
    }
    plan_toml.push_str(&format!(
        "description = \"{}\"\n",
        plan.metadata.description
    ));
    plan_toml.push_str(&format!("arch = \"{}\"\n", plan.metadata.arch));
    plan_toml.push_str(&format!("license = \"{}\"\n", plan.metadata.license));

    if !plan.build_deps.is_empty() {
        plan_toml.push_str("build_deps = [");
        for (i, d) in plan.build_deps.iter().enumerate() {
            if i > 0 {
                plan_toml.push_str(", ");
            }
            plan_toml.push_str(&format!("\"{}\"", d));
        }
        plan_toml.push_str("]\n");
    }
    if !plan.link_deps.is_empty() {
        plan_toml.push_str("link_deps = [");
        for (i, d) in plan.link_deps.iter().enumerate() {
            if i > 0 {
                plan_toml.push_str(", ");
            }
            plan_toml.push_str(&format!("\"{}\"", d));
        }
        plan_toml.push_str("]\n");
    }

    format!(
        r#"[part]
name = "{name}"
build_date = "{build_date}"
packager = "wright {wright_version}"
{runtime_deps}{relations}{backup}{plan}
"#,
        name = manifest.metadata.name,
        build_date = build_date,
        wright_version = env!("CARGO_PKG_VERSION"),
        runtime_deps = runtime_deps_toml,
        relations = relations_toml,
        backup = backup_toml,
        plan = plan_toml,
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
        relations: Option<PartInfoRelations>,
        #[serde(default)]
        backup: Option<PartInfoBackup>,
    }

    #[derive(serde::Deserialize)]
    struct PartInfoMeta {
        name: String,
        #[serde(default)]
        build_date: String,
        #[serde(default)]
        runtime_deps: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    struct PartInfoPlan {
        name: String,
        #[serde(default)]
        version: String,
        release: u32,
        #[serde(default)]
        epoch: u32,
        description: String,
        arch: String,
        license: String,
        #[serde(default)]
        build_deps: Vec<String>,
        #[serde(default)]
        link_deps: Vec<String>,
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

    let relations = parsed.relations.unwrap_or_default();
    let plan_section = parsed.plan.ok_or_else(|| {
        WrightError::PartError(".PARTINFO missing required [plan] section".to_string())
    })?;

    Ok(PartInfo {
        name: parsed.part.name,
        build_date: parsed.part.build_date,
        runtime_deps: parsed.part.runtime_deps,
        replaces: relations.replaces,
        conflicts: relations.conflicts,
        provides: relations.provides,
        backup_files: parsed.backup.map(|b| b.files).unwrap_or_default(),
        plan: PlanMetadata {
            name: plan_section.name,
            version: plan_section.version,
            release: plan_section.release,
            epoch: plan_section.epoch,
            description: plan_section.description,
            arch: plan_section.arch,
            license: plan_section.license,
            build_deps: plan_section.build_deps,
            link_deps: plan_section.link_deps,
        },
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
version = "1.0.0"
release = 1
description = "demo"
arch = "x86_64"
license = "MIT"
runtime_deps = ["bash"]

[plan]
name = "demo"
version = "1.0.0"
release = 1
description = "demo"
arch = "x86_64"
license = "MIT"
"#,
        )
        .unwrap();

        assert_eq!(info.runtime_deps, vec!["bash"]);
        assert_eq!(info.plan.name, "demo");
        assert_eq!(info.name, "demo");
    }

    #[test]
    fn parse_partinfo_with_plan_section() {
        let info = parse_partinfo_str(
            r#"
[part]
name = "libstdc++"
runtime_deps = ["libgcc:default"]

[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "GNU Compiler Collection"
arch = "x86_64"
license = "GPL-3.0-or-later"
build_deps = ["binutils:default"]
link_deps = ["zlib:default"]
"#,
        )
        .unwrap();

        assert_eq!(info.name, "libstdc++");
        assert_eq!(info.plan.name, "gcc");
        assert_eq!(info.plan.version, "14.2.0");
        assert_eq!(info.plan.release, 1);
        assert_eq!(info.runtime_deps, vec!["libgcc:default"]);
        assert_eq!(info.plan.build_deps, vec!["binutils:default"]);
        assert_eq!(info.plan.link_deps, vec!["zlib:default"]);
    }

    #[test]
    fn parse_partinfo_missing_plan_section_fails() {
        let result = parse_partinfo_str(
            r#"
[part]
name = "demo"
build_date = "2025-01-01"
runtime_deps = ["bash"]
"#,
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing required [plan]"));
    }
}
