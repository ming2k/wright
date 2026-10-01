use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};

/// Plan-level metadata extracted from the `[plan]` section of `.PARTINFO`.
/// All outputs of a plan share these fields; they are stored in the `plans` table.
///
/// Only identity + runtime-discriminator fields are carried here.
/// Human-readable documentation (`description`, `license`, `url`) lives in
/// plan source only and is not duplicated into binary part metadata.
#[derive(Debug, Clone)]
pub struct PlanMetadata {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub arch: String,
}

/// Seal-time provenance from the `[provenance]` section of `.PARTINFO`.
///
/// Descriptive facts, never enforced (ADR-0023): they let the ledger tie a
/// part back to the plan content and sources that produced it, and let
/// `wright doctor` detect drift between an installed part and current plan
/// source. Parts sealed before ADR-0023 do not carry the section.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// SHA-256 of the raw plan.toml that produced the part.
    pub plan_checksum: Option<String>,
    /// One line per `[[sources]]` entry: kind, expanded locator, and the
    /// verification declared for it (e.g. `http <url> sha256=<hash>`).
    pub source_checksums: Vec<String>,
    /// Version of the `wright` binary that sealed the part.
    pub wright_version: String,
    /// Weakest effective isolation level across the plan's pipeline stages.
    pub isolation: String,
}

/// Deployment hooks serialized into an archive's `.HOOKS` file.
#[derive(Debug, Clone, Default)]
pub struct PartHooks {
    pub pre_install: Option<String>,
    pub post_install: Option<String>,
    pub post_upgrade: Option<String>,
    pub pre_remove: Option<String>,
    pub post_remove: Option<String>,
}

/// Build-host audit data serialized into an archive's `.BUILDINFO` file.
///
/// Unlike `.PARTINFO` (install-time metadata), `.BUILDINFO` is pure
/// forensics: when a part misbehaves after deployment, it answers "what
/// machine produced this?" — CPU model and flags expose microarchitecture
/// mismatches, kernel/hostname tie the artifact to a build environment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildInfo {
    /// Version of the `wright` binary that sealed the part (duplicated from
    /// `.PARTINFO` provenance so this file is self-contained forensics).
    pub wright_version: String,
    pub host: crate::platform::HostInfo,
}

/// Archive-format input used when sealing a part.
///
/// Callers project their source manifest into this type before crossing the
/// archive boundary. That keeps archive encoding independent of any specific
/// manifest representation.
#[derive(Debug, Clone)]
pub struct PartSpec {
    pub archive_name: String,
    pub name: String,
    pub runtime_deps: Vec<String>,
    pub replaces: Vec<String>,
    pub conflicts: Vec<String>,
    pub backup_files: Vec<String>,
    pub plan: PlanMetadata,
    pub provenance: Provenance,
    /// Raw plan.toml text to embed as the archive's `.PLANSRC` member
    /// (ADR-0033). `None` seals no snapshot — readers must treat the member
    /// as optional, like pre-ADR-0023 provenance.
    pub plan_source: Option<String>,
    /// Build-host audit data to embed as the archive's `.BUILDINFO` member.
    /// Optional like `plan_source`: archives sealed before it existed simply
    /// lack the member.
    pub build_info: Option<BuildInfo>,
    pub hooks: PartHooks,
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
    pub backup_files: Vec<String>,
    pub plan: PlanMetadata,
    pub provenance: Option<Provenance>,
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

/// Canonical list of archive-level protocol metadata files situated at the archive root.
/// These files carry archive structure, provenance, and hooks; they are never payload
/// and must never be installed to the target filesystem or recorded in .FILELIST.
pub const ARCHIVE_METADATA_FILES: &[&str] = &[
    ".PARTINFO",
    ".FILELIST",
    ".HOOKS",
    ".PLANSRC",
    ".BUILDINFO",
    ".ABIINFO",
];

/// Returns true if the given relative or normalized path points to an archive-level
/// protocol metadata file.
///
/// Protocol metadata strictly resides at the root level of the archive or staging tree.
/// A file with the same name located in a subdirectory (e.g. `etc/.ABIINFO`) is normal
/// payload, not protocol metadata.
///
/// Leading root `/` or current-dir `./` components are ignored, allowing both
/// relative paths (e.g. `.ABIINFO`, `./.ABIINFO`) and normalized root paths
/// (e.g. `/.ABIINFO`) to be checked reliably.
pub fn is_archive_metadata(path: &Path) -> bool {
    let mut components = path.components().filter(|c| {
        !matches!(c, std::path::Component::RootDir | std::path::Component::CurDir)
    });
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => {
            name.to_str().is_some_and(|s| ARCHIVE_METADATA_FILES.contains(&s))
        }
        _ => false,
    }
}

/// Returns true if the path inside a tar archive matches the target metadata file name
/// situated at the archive root.
pub fn is_archive_member(path: &Path, expected_name: &str) -> bool {
    let mut components = path.components().filter(|c| {
        !matches!(c, std::path::Component::RootDir | std::path::Component::CurDir)
    });
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => name == expected_name,
        _ => false,
    }
}

/// Remove all archive protocol metadata files from the root of the given staging directory.
pub fn clean_staging_metadata(part_dir: &Path) {
    for name in ARCHIVE_METADATA_FILES {
        let _ = std::fs::remove_file(part_dir.join(name));
    }
}

/// Write a `.wright.tar.zst` binary part archive from archive-owned metadata.
pub fn write_part(part_dir: &Path, spec: &PartSpec, output_path: &Path) -> Result<PathBuf> {
    let mut components = Path::new(&spec.archive_name).components();
    let valid_archive_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if !valid_archive_name {
        return Err(WrightError::ValidationError(format!(
            "invalid part archive filename: {}",
            spec.archive_name
        )));
    }

    purge_excluded_files(part_dir);

    // A previously interrupted seal must not leak stale metadata into the
    // next archive.
    clean_staging_metadata(part_dir);

    let partinfo_path = part_dir.join(".PARTINFO");
    let filelist_path = part_dir.join(".FILELIST");
    let hooks_path = part_dir.join(".HOOKS");
    let plansrc_path = part_dir.join(".PLANSRC");
    let buildinfo_path = part_dir.join(".BUILDINFO");
    let abiinfo_path = part_dir.join(".ABIINFO");

    // Generate .PARTINFO
    let partinfo = generate_partinfo(spec);

    // Generate .FILELIST
    let filelist = generate_filelist(part_dir)?;

    // An empty staging tree means the forge produced nothing (stale
    // checkpoint, cleaned workshop, broken install stage).  Sealing it would
    // publish a part that deploys zero files — fail loudly instead.
    if filelist.trim().is_empty() {
        return Err(WrightError::PartError(format!(
            "refusing to seal '{}': staging tree {} contains no files \
             (re-run the forge with --force --clean)",
            spec.name,
            part_dir.display()
        )));
    }

    let result = (|| {
        std::fs::write(&partinfo_path, &partinfo)
            .map_err(|e| WrightError::context("failed to write .PARTINFO", e))?;

        std::fs::write(&filelist_path, &filelist)
            .map_err(|e| WrightError::context("failed to write .FILELIST", e))?;

        let hooks_content = generate_hooks_toml(&spec.hooks);
        if !hooks_content.is_empty() {
            std::fs::write(&hooks_path, &hooks_content)
                .map_err(|e| WrightError::context("failed to write .HOOKS", e))?;
        }

        if let Some(ref plan_source) = spec.plan_source {
            std::fs::write(&plansrc_path, plan_source)
                .map_err(|e| WrightError::context("failed to write .PLANSRC", e))?;
        }

        if let Some(ref build_info) = spec.build_info {
            let content = toml::to_string(build_info)
                .map_err(|e| WrightError::context("failed to serialize .BUILDINFO", e))?;
            std::fs::write(&buildinfo_path, content)
                .map_err(|e| WrightError::context("failed to write .BUILDINFO", e))?;
        }

        let part_abi = crate::abi::extract_part_abi(part_dir)?;
        if !part_abi.libraries.is_empty() {
            crate::abi::write_abi_info(&abiinfo_path, &part_abi)?;
        }

        let part_path = output_path.join(&spec.archive_name);
        crate::compression::create_tar_zst(part_dir, &part_path)?;
        Ok(part_path)
    })();

    // Metadata belongs to the archive, never to the staging tree. Clean it up
    // after both successful and failed archive writes.
    clean_staging_metadata(part_dir);

    result
}

/// Extract a .wright.tar.zst archive and return the parsed PARTINFO along with
/// the SHA-256 hash of the archive file, computed in a single streaming pass.
pub fn extract_part(part_path: &Path, dest_dir: &Path) -> Result<(PartInfo, String)> {
    let hash = crate::compression::extract_tar_zst_hashed(part_path, dest_dir)?;
    let partinfo_path = dest_dir.join(".PARTINFO");
    if partinfo_path.exists() {
        return Ok((parse_partinfo(&partinfo_path)?, hash));
    }

    Err(WrightError::PartError(format!(
        "{}: archive does not contain .PARTINFO",
        part_path.display()
    )))
}

/// Read the `.PLANSRC` snapshot from an already-extracted archive directory.
/// Returns `None` for archives sealed before ADR-0033 (or from manifests not
/// loaded from a file); the member is optional by contract.
pub fn read_plan_source(extract_dir: &Path) -> Option<String> {
    std::fs::read_to_string(extract_dir.join(".PLANSRC")).ok()
}

/// Read the `.BUILDINFO` audit data from an already-extracted archive
/// directory. Returns `None` for archives sealed before `.BUILDINFO`
/// existed; the member is optional by contract, like `.PLANSRC`.
pub fn read_build_info(extract_dir: &Path) -> Option<BuildInfo> {
    let content = std::fs::read_to_string(extract_dir.join(".BUILDINFO")).ok()?;
    toml::from_str(&content).ok()
}

/// Read the `.ABIINFO` snapshot from an already-extracted archive directory.
pub fn read_abi_info(extract_dir: &Path) -> Option<crate::abi::PartAbi> {
    crate::abi::read_abi_info(extract_dir).ok().flatten()
}

/// Light summary of an archive's metadata + file list, used by the
/// package-time ELF lint to build SONAME → part lookups without full
/// extraction.
pub struct ArchiveMeta {
    pub partinfo: PartInfo,
    pub files: Vec<String>,
}

/// Read both .PARTINFO and .FILELIST from an archive in a single streamed
/// pass. .FILELIST entries are returned verbatim (one path per non-empty
/// line, leading/trailing whitespace trimmed).
pub fn read_archive_meta(part_path: &Path) -> Result<ArchiveMeta> {
    let file = std::fs::File::open(part_path)
        .map_err(|e| WrightError::context(format!("failed to open {}", part_path.display()), e))?;
    let decoder = zstd::Decoder::new(file)
        .map_err(|e| WrightError::context("zstd decoder init failed", e))?;
    let mut archive = tar::Archive::new(decoder);

    let mut partinfo: Option<PartInfo> = None;
    let mut files: Option<Vec<String>> = None;

    for entry in archive
        .entries()
        .map_err(|e| WrightError::context("failed to read archive entries", e))?
    {
        let mut entry = entry.map_err(|e| WrightError::context("failed to read entry", e))?;
        let path = entry
            .path()
            .map_err(|e| WrightError::context("failed to read entry path", e))?;

        if is_archive_member(&path, ".PARTINFO") && partinfo.is_none() {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::context("failed to read .PARTINFO", e))?;
            partinfo = Some(parse_partinfo_str(
                &content,
                &part_path.display().to_string(),
            )?);
        } else if is_archive_member(&path, ".FILELIST") && files.is_none() {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::context("failed to read .FILELIST", e))?;
            files = Some(
                content
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect(),
            );
        }

        if partinfo.is_some() && files.is_some() {
            break;
        }
    }

    let partinfo = partinfo.ok_or_else(|| {
        WrightError::PartError(format!(
            "{}: archive does not contain .PARTINFO",
            part_path.display()
        ))
    })?;
    Ok(ArchiveMeta {
        partinfo,
        files: files.unwrap_or_default(),
    })
}

/// Read .PARTINFO from an archive without full extraction.
pub fn read_partinfo(part_path: &Path) -> Result<PartInfo> {
    let file = std::fs::File::open(part_path)
        .map_err(|e| WrightError::context(format!("failed to open {}", part_path.display()), e))?;

    let decoder = zstd::Decoder::new(file)
        .map_err(|e| WrightError::context("zstd decoder init failed", e))?;

    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| WrightError::context("failed to read archive entries", e))?
    {
        let mut entry = entry.map_err(|e| WrightError::context("failed to read entry", e))?;

        let path = entry
            .path()
            .map_err(|e| WrightError::context("failed to read entry path", e))?;

        if is_archive_member(&path, ".PARTINFO") {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::context("failed to read .PARTINFO", e))?;
            return parse_partinfo_str(&content, &part_path.display().to_string());
        }
    }

    Err(WrightError::PartError(format!(
        "{}: archive does not contain .PARTINFO",
        part_path.display()
    )))
}

/// Read .PLANSRC from an archive without full extraction, returning None if absent.
pub fn read_archive_plansrc(part_path: &Path) -> Result<Option<String>> {
    let file = std::fs::File::open(part_path)
        .map_err(|e| WrightError::context(format!("failed to open {}", part_path.display()), e))?;

    let decoder = zstd::Decoder::new(file)
        .map_err(|e| WrightError::context("zstd decoder init failed", e))?;

    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| WrightError::context("failed to read archive entries", e))?
    {
        let mut entry = entry.map_err(|e| WrightError::context("failed to read entry", e))?;

        let path = entry
            .path()
            .map_err(|e| WrightError::context("failed to read entry path", e))?;

        if is_archive_member(&path, ".PLANSRC") {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::context("failed to read .PLANSRC", e))?;
            return Ok(Some(content));
        }
    }

    Ok(None)
}

fn generate_partinfo(spec: &PartSpec) -> String {
    let build_date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut runtime_deps_toml = String::new();
    if !spec.runtime_deps.is_empty() {
        runtime_deps_toml.push_str("runtime_deps = [");
        for (i, dep) in spec.runtime_deps.iter().enumerate() {
            if i > 0 {
                runtime_deps_toml.push_str(", ");
            }
            runtime_deps_toml.push_str(&format!("\"{}\"", dep));
        }
        runtime_deps_toml.push_str("]\n");
    }

    let mut relations_toml = String::new();
    if !spec.replaces.is_empty() || !spec.conflicts.is_empty() {
        relations_toml.push_str("\n[relations]\n");
        if !spec.replaces.is_empty() {
            relations_toml.push_str("replaces = [");
            for (i, dep) in spec.replaces.iter().enumerate() {
                if i > 0 {
                    relations_toml.push_str(", ");
                }
                relations_toml.push_str(&format!("\"{}\"", dep));
            }
            relations_toml.push_str("]\n");
        }
        if !spec.conflicts.is_empty() {
            relations_toml.push_str("conflicts = [");
            for (i, dep) in spec.conflicts.iter().enumerate() {
                if i > 0 {
                    relations_toml.push_str(", ");
                }
                relations_toml.push_str(&format!("\"{}\"", dep));
            }
            relations_toml.push_str("]\n");
        }
    }

    let mut backup_toml = String::new();
    if !spec.backup_files.is_empty() {
        backup_toml.push_str("\n[backup]\nfiles = [");
        for (i, f) in spec.backup_files.iter().enumerate() {
            if i > 0 {
                backup_toml.push_str(", ");
            }
            backup_toml.push_str(&format!("\"{}\"", f));
        }
        backup_toml.push_str("]\n");
    }

    let mut plan_toml = String::new();
    plan_toml.push_str("\n[plan]\n");
    plan_toml.push_str(&format!("name = \"{}\"\n", spec.plan.name));
    if !spec.plan.version.is_empty() {
        plan_toml.push_str(&format!("version = \"{}\"\n", spec.plan.version));
    }
    plan_toml.push_str(&format!("release = {}\n", spec.plan.release));
    if spec.plan.epoch > 0 {
        plan_toml.push_str(&format!("epoch = {}\n", spec.plan.epoch));
    }
    plan_toml.push_str(&format!("arch = \"{}\"\n", spec.plan.arch));

    format!(
        r#"[part]
name = "{name}"
build_date = "{build_date}"
packager = "wright {wright_version}"
{runtime_deps}{relations}{backup}{plan}{provenance}
"#,
        name = spec.name,
        build_date = build_date,
        wright_version = env!("CARGO_PKG_VERSION"),
        runtime_deps = runtime_deps_toml,
        relations = relations_toml,
        backup = backup_toml,
        plan = plan_toml,
        provenance = generate_provenance_toml(&spec.provenance),
    )
}

/// Render the `[provenance]` section from the plan-level manifest (ADR-0023).
fn generate_provenance_toml(provenance: &Provenance) -> String {
    let mut toml = String::from("\n[provenance]\n");
    if let Some(ref sum) = provenance.plan_checksum {
        toml.push_str(&format!("plan_checksum = \"{}\"\n", sum));
    }
    if !provenance.source_checksums.is_empty() {
        toml.push_str("source_checksums = [\n");
        for source in &provenance.source_checksums {
            toml.push_str(&format!("    \"{}\",\n", source));
        }
        toml.push_str("]\n");
    }
    toml.push_str(&format!(
        "wright_version = \"{}\"\n",
        provenance.wright_version
    ));
    toml.push_str(&format!("isolation = \"{}\"\n", provenance.isolation));
    toml
}

pub fn generate_filelist(part_dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    for entry in WalkDir::new(part_dir).sort_by_file_name() {
        let entry = entry.map_err(|e| WrightError::context("failed to walk directory", e))?;
        let relative = entry.path().strip_prefix(part_dir).unwrap_or(entry.path());
        // Skip metadata files and root
        if relative.as_os_str().is_empty() || is_archive_metadata(relative) {
            continue;
        }
        files.push(format!("/{}", relative.to_string_lossy()));
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
fn generate_hooks_toml(scripts: &PartHooks) -> String {
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
        if let Some(s) = value {
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
    let content = std::fs::read_to_string(path).map_err(|e| {
        WrightError::context(format!("{}: failed to read .PARTINFO", path.display()), e)
    })?;
    parse_partinfo_str(&content, &path.display().to_string())
}

fn parse_partinfo_str(content: &str, source: &str) -> Result<PartInfo> {
    #[derive(serde::Deserialize)]
    struct PartInfoToml {
        part: PartInfoMeta,
        #[serde(default)]
        plan: Option<PartInfoPlan>,
        #[serde(default)]
        relations: Option<PartInfoRelations>,
        #[serde(default)]
        backup: Option<PartInfoBackup>,
        #[serde(default)]
        provenance: Option<PartInfoProvenance>,
    }

    #[derive(serde::Deserialize)]
    struct PartInfoProvenance {
        #[serde(default)]
        plan_checksum: Option<String>,
        #[serde(default)]
        source_checksums: Vec<String>,
        #[serde(default)]
        wright_version: String,
        #[serde(default)]
        isolation: String,
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
        arch: String,
    }

    #[derive(serde::Deserialize, Default)]
    struct PartInfoRelations {
        #[serde(default)]
        replaces: Vec<String>,
        #[serde(default)]
        conflicts: Vec<String>,
    }

    #[derive(serde::Deserialize)]
    struct PartInfoBackup {
        #[serde(default)]
        files: Vec<String>,
    }

    let parsed: PartInfoToml = toml::from_str(content)
        .map_err(|e| WrightError::context(format!("{}: failed to parse .PARTINFO", source), e))?;

    let relations = parsed.relations.unwrap_or_default();
    let plan_section = parsed.plan.ok_or_else(|| {
        WrightError::PartError(format!(
            "{}: .PARTINFO missing required [plan] section",
            source
        ))
    })?;

    Ok(PartInfo {
        name: parsed.part.name,
        build_date: parsed.part.build_date,
        runtime_deps: parsed.part.runtime_deps,
        replaces: relations.replaces,
        conflicts: relations.conflicts,
        backup_files: parsed.backup.map(|b| b.files).unwrap_or_default(),
        plan: PlanMetadata {
            name: plan_section.name,
            version: plan_section.version,
            release: plan_section.release,
            epoch: plan_section.epoch,
            arch: plan_section.arch,
        },
        provenance: parsed.provenance.map(|p| Provenance {
            plan_checksum: p.plan_checksum,
            source_checksums: p.source_checksums,
            wright_version: p.wright_version,
            isolation: p.isolation,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PartHooks, PartSpec, PlanMetadata, Provenance, generate_partinfo, parse_partinfo_str,
    };

    fn part_spec(isolation: &str) -> PartSpec {
        PartSpec {
            archive_name: "demo-1.2.3-1-x86_64.wright.tar.zst".to_string(),
            name: "demo".to_string(),
            runtime_deps: Vec::new(),
            replaces: Vec::new(),
            conflicts: Vec::new(),
            backup_files: Vec::new(),
            plan: PlanMetadata {
                name: "demo".to_string(),
                version: "1.2.3".to_string(),
                release: 1,
                epoch: 0,
                arch: "x86_64".to_string(),
            },
            provenance: Provenance {
                plan_checksum: Some("deadbeef".to_string()),
                source_checksums: vec![
                    "http https://example.org/demo-1.2.3.tar.gz sha256=abc123".to_string(),
                    "git https://example.org/demo.git ref=v1.2.3".to_string(),
                ],
                wright_version: env!("CARGO_PKG_VERSION").to_string(),
                isolation: isolation.to_string(),
            },
            plan_source: None,
            build_info: None,
            hooks: PartHooks::default(),
        }
    }

    #[test]
    fn create_part_refuses_empty_staging_tree() {
        let spec = part_spec("strict");
        let staging = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();

        // An empty staging tree must not seal (regression: wright 5.0.2
        // packed metadata-only parts that deployed zero files).
        let err = super::write_part(staging.path(), &spec, out.path()).unwrap_err();
        assert!(err.to_string().contains("contains no files"), "{err}");
        assert!(!out.path().join(&spec.archive_name).exists());

        // The same tree with payload seals fine.
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let part = super::write_part(staging.path(), &spec, out.path()).unwrap();
        assert!(part.exists());
    }

    #[test]
    fn write_part_rejects_archive_path_components() {
        let mut spec = part_spec("strict");
        spec.archive_name = "../demo.wright.tar.zst".to_string();
        let staging = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();

        let err = super::write_part(staging.path(), &spec, out.path()).unwrap_err();
        assert!(err.to_string().contains("invalid part archive filename"));
    }

    #[test]
    fn plan_source_seals_as_plansrc_member() {
        let mut spec = part_spec("strict");
        spec.plan_source = Some("name = \"demo\"\nrelease = 1\n".to_string());
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let out = tempfile::tempdir().unwrap();

        let part = super::write_part(staging.path(), &spec, out.path()).unwrap();

        // The snapshot survives in the archive, stays out of .FILELIST, and
        // is cleaned from the staging tree after sealing.
        let extract = tempfile::tempdir().unwrap();
        let (_info, _hash) = super::extract_part(&part, extract.path()).unwrap();
        assert_eq!(
            super::read_plan_source(extract.path()).as_deref(),
            Some("name = \"demo\"\nrelease = 1\n")
        );
        assert_eq!(
            super::read_archive_plansrc(&part).unwrap().as_deref(),
            Some("name = \"demo\"\nrelease = 1\n")
        );
        let meta = super::read_archive_meta(&part).unwrap();
        assert!(
            !meta.files.iter().any(|f| f.contains(".PLANSRC")),
            ".PLANSRC must not appear in .FILELIST: {:?}",
            meta.files
        );
        assert!(!staging.path().join(".PLANSRC").exists());
    }

    #[test]
    fn plan_source_absent_means_no_plansrc_member() {
        let spec = part_spec("strict"); // plan_source: None
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let out = tempfile::tempdir().unwrap();

        let part = super::write_part(staging.path(), &spec, out.path()).unwrap();
        let extract = tempfile::tempdir().unwrap();
        let _ = super::extract_part(&part, extract.path()).unwrap();
        assert!(super::read_plan_source(extract.path()).is_none());
        assert!(super::read_archive_plansrc(&part).unwrap().is_none());
    }

    #[test]
    fn build_info_seals_as_buildinfo_member() {
        let mut spec = part_spec("strict");
        spec.build_info = Some(super::BuildInfo {
            wright_version: env!("CARGO_PKG_VERSION").to_string(),
            host: crate::platform::HostInfo::probe(true),
        });
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let out = tempfile::tempdir().unwrap();

        let part = super::write_part(staging.path(), &spec, out.path()).unwrap();

        // The audit data round-trips through the archive, stays out of
        // .FILELIST, and is cleaned from the staging tree after sealing.
        let extract = tempfile::tempdir().unwrap();
        let _ = super::extract_part(&part, extract.path()).unwrap();
        let info = super::read_build_info(extract.path()).expect("embedded .BUILDINFO");
        assert!(!info.host.cpu_model.is_empty());
        assert!(info.host.cpu_cores >= 1);
        let meta = super::read_archive_meta(&part).unwrap();
        assert!(
            !meta.files.iter().any(|f| f.contains(".BUILDINFO")),
            ".BUILDINFO must not appear in .FILELIST: {:?}",
            meta.files
        );
        assert!(!staging.path().join(".BUILDINFO").exists());
    }

    #[test]
    fn build_info_absent_means_no_buildinfo_member() {
        let spec = part_spec("strict"); // build_info: None
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let out = tempfile::tempdir().unwrap();

        let part = super::write_part(staging.path(), &spec, out.path()).unwrap();
        let extract = tempfile::tempdir().unwrap();
        let _ = super::extract_part(&part, extract.path()).unwrap();
        assert!(super::read_build_info(extract.path()).is_none());
    }

    #[test]
    fn write_part_cleans_metadata_after_archive_failure() {
        let spec = part_spec("strict");
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let missing_output = staging.path().join("missing/output");

        super::write_part(staging.path(), &spec, &missing_output).unwrap_err();

        for name in super::ARCHIVE_METADATA_FILES {
            assert!(
                !staging.path().join(name).exists(),
                "archive failure must clean {} from staging",
                name
            );
        }
    }

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
            "test",
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
runtime_deps = ["libgcc"]

[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "GNU Compiler Collection"
arch = "x86_64"
license = "GPL-3.0-or-later"
"#,
            "test",
        )
        .unwrap();

        assert_eq!(info.name, "libstdc++");
        assert_eq!(info.plan.name, "gcc");
        assert_eq!(info.plan.version, "14.2.0");
        assert_eq!(info.plan.release, 1);
        assert_eq!(info.runtime_deps, vec!["libgcc"]);
    }

    #[test]
    fn parse_partinfo_without_provenance_is_none() {
        let info = parse_partinfo_str(
            r#"
[part]
name = "demo"

[plan]
name = "demo"
version = "1.0.0"
release = 1
arch = "x86_64"
"#,
            "test",
        )
        .unwrap();

        assert!(info.provenance.is_none());
    }

    #[test]
    fn provenance_roundtrips_through_generated_partinfo() {
        let partinfo = generate_partinfo(&part_spec("none"));
        let info = parse_partinfo_str(&partinfo, "test").unwrap();

        let provenance = info.provenance.expect("generated .PARTINFO has provenance");
        assert_eq!(provenance.plan_checksum.as_deref(), Some("deadbeef"));
        assert_eq!(
            provenance.source_checksums,
            vec![
                "http https://example.org/demo-1.2.3.tar.gz sha256=abc123",
                "git https://example.org/demo.git ref=v1.2.3",
            ]
        );
        assert_eq!(provenance.wright_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(provenance.isolation, "none");
    }

    #[test]
    fn provenance_uses_engine_resolved_isolation() {
        let partinfo = generate_partinfo(&part_spec("relaxed"));
        let info = parse_partinfo_str(&partinfo, "test").unwrap();
        assert_eq!(info.provenance.unwrap().isolation, "relaxed");
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
            "test",
        );

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing required [plan]")
        );
    }

    #[test]
    fn metadata_ssot_exact_matching_and_invariants() {
        use std::path::Path;

        // Positive matches: root-level with or without leading slash / curdir
        for name in super::ARCHIVE_METADATA_FILES {
            assert!(
                super::is_archive_metadata(Path::new(name)),
                "{} must be recognized as archive metadata",
                name
            );
            assert!(
                super::is_archive_metadata(Path::new(&format!("/{}", name))),
                "/{} must be recognized as archive metadata",
                name
            );
            assert!(
                super::is_archive_metadata(Path::new(&format!("./{}", name))),
                "./{} must be recognized as archive metadata",
                name
            );
            assert!(
                super::is_archive_member(Path::new(name), name),
                "is_archive_member must match exact name {}",
                name
            );
            assert!(
                super::is_archive_member(Path::new(&format!("./{}", name)), name),
                "is_archive_member must match ./{}",
                name
            );
        }

        // Subdirectories: same name in subdirectory must NOT be treated as archive metadata
        let subpaths = [
            "etc/.ABIINFO",
            "usr/share/doc/.PARTINFO",
            "var/lib/.PLANSRC",
            "/etc/.FILELIST",
            "/usr/share/.BUILDINFO",
            "/opt/.HOOKS",
        ];
        for subpath in &subpaths {
            assert!(
                !super::is_archive_metadata(Path::new(subpath)),
                "nested file {} must NOT be treated as archive metadata",
                subpath
            );
            assert!(
                !super::is_archive_member(Path::new(subpath), ".ABIINFO"),
                "nested file {} must NOT match archive member",
                subpath
            );
        }

        // Partial prefix / suffix collisions
        let collisions = [
            ".PARTINFO_foo",
            ".FILELIST.bak",
            ".HOOKS_old",
            ".PLANSRC.txt",
            ".BUILDINFO_v2",
            ".ABIINFO_extra",
            "usr/bin/app",
            "/bin/sh",
        ];
        for col in &collisions {
            assert!(
                !super::is_archive_metadata(Path::new(col)),
                "{} must NOT match archive metadata",
                col
            );
        }
    }

    #[test]
    fn sealing_invariant_excludes_and_cleans_all_archive_metadata() {
        let spec = part_spec("strict");
        let staging = tempfile::tempdir().unwrap();

        // Populate valid payload
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/app"), "binary").unwrap();
        std::fs::create_dir_all(staging.path().join("etc")).unwrap();
        std::fs::write(staging.path().join("etc/app.conf"), "config").unwrap();
        // A payload file that shares the name of a metadata file in a nested folder
        std::fs::create_dir_all(staging.path().join("usr/share")).unwrap();
        std::fs::write(staging.path().join("usr/share/.ABIINFO"), "nested").unwrap();

        // Stale metadata sitting in staging root from interrupted build
        for name in super::ARCHIVE_METADATA_FILES {
            std::fs::write(staging.path().join(name), "stale").unwrap();
        }

        // 1. generate_filelist must exclude all root metadata but preserve nested payload
        let filelist = super::generate_filelist(staging.path()).unwrap();
        let filelist_lines: Vec<&str> = filelist.lines().collect();

        assert!(filelist_lines.contains(&"/usr/bin/app"));
        assert!(filelist_lines.contains(&"/etc/app.conf"));
        assert!(
            filelist_lines.contains(&"/usr/share/.ABIINFO"),
            "nested file with same name must be retained in filelist"
        );
        for name in super::ARCHIVE_METADATA_FILES {
            let root_meta = format!("/{}", name);
            assert!(
                !filelist_lines.contains(&root_meta.as_str()),
                "{} must NOT appear in generated .FILELIST",
                root_meta
            );
        }

        // 2. Seal archive
        let out = tempfile::tempdir().unwrap();
        let part = super::write_part(staging.path(), &spec, out.path()).unwrap();

        // 3. Staging directory must be cleaned of all root metadata
        for name in super::ARCHIVE_METADATA_FILES {
            assert!(
                !staging.path().join(name).exists(),
                "staging root must be cleaned of {} after sealing",
                name
            );
        }
        // But nested payload must remain
        assert!(staging.path().join("usr/share/.ABIINFO").exists());
        assert!(staging.path().join("usr/bin/app").exists());

        // 4. Archive .FILELIST must not have metadata
        let meta = super::read_archive_meta(&part).unwrap();
        for name in super::ARCHIVE_METADATA_FILES {
            let root_meta = format!("/{}", name);
            assert!(
                !meta.files.contains(&root_meta),
                "sealed archive filelist must not contain {}",
                root_meta
            );
        }
    }
}
