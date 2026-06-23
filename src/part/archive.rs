use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::Utc;
use walkdir::WalkDir;

use crate::error::{Result, WrightError};
use crate::plan::manifest::{PlanManifest, Source};

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
    /// Weakest isolation level declared across the plan's pipeline stages.
    pub isolation: String,
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

    // An empty staging tree means the forge produced nothing (stale
    // checkpoint, cleaned workshop, broken install stage).  Sealing it would
    // publish a part that deploys zero files — fail loudly instead.
    if filelist.trim().is_empty() {
        return Err(WrightError::PartError(format!(
            "refusing to seal '{}': staging tree {} contains no files \
             (re-run the forge with --force --clean)",
            manifest.metadata.name,
            part_dir.display()
        )));
    }

    // Write metadata files into part_dir
    std::fs::write(part_dir.join(".PARTINFO"), &partinfo)
        .map_err(|e| WrightError::PartError(format!("failed to write .PARTINFO: {}", e)))?;

    std::fs::write(part_dir.join(".FILELIST"), &filelist)
        .map_err(|e| WrightError::PartError(format!("failed to write .FILELIST: {}", e)))?;

    // Write .HOOKS (TOML) if install scripts exist
    if let Some(ref scripts) = manifest.deploy_scripts {
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

    Err(WrightError::PartError(format!(
        "{}: archive does not contain .PARTINFO",
        part_path.display()
    )))
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
    let file = std::fs::File::open(part_path).map_err(|e| {
        WrightError::PartError(format!("failed to open {}: {}", part_path.display(), e))
    })?;
    let decoder = zstd::Decoder::new(file)
        .map_err(|e| WrightError::PartError(format!("zstd decoder init failed: {}", e)))?;
    let mut archive = tar::Archive::new(decoder);

    let mut partinfo: Option<PartInfo> = None;
    let mut files: Option<Vec<String>> = None;

    for entry in archive
        .entries()
        .map_err(|e| WrightError::PartError(format!("failed to read archive entries: {}", e)))?
    {
        let mut entry =
            entry.map_err(|e| WrightError::PartError(format!("failed to read entry: {}", e)))?;
        let path = entry
            .path()
            .map_err(|e| WrightError::PartError(format!("failed to read entry path: {}", e)))?;
        let path_str = path.to_string_lossy().into_owned();

        if path_str.ends_with(".PARTINFO") && partinfo.is_none() {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::PartError(format!("failed to read .PARTINFO: {}", e)))?;
            partinfo = Some(parse_partinfo_str(
                &content,
                &part_path.display().to_string(),
            )?);
        } else if path_str.ends_with(".FILELIST") && files.is_none() {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| WrightError::PartError(format!("failed to read .FILELIST: {}", e)))?;
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
            return parse_partinfo_str(&content, &part_path.display().to_string());
        }
    }

    Err(WrightError::PartError(format!(
        "{}: archive does not contain .PARTINFO",
        part_path.display()
    )))
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
    if !manifest.relations.replaces.is_empty() || !manifest.relations.conflicts.is_empty() {
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
    }

    let mut backup_toml = String::new();
    if let Some(ref backup) = manifest.backup
        && !backup.files.is_empty()
    {
        backup_toml.push_str("\n[backup]\nfiles = [");
        for (i, f) in backup.files.iter().enumerate() {
            if i > 0 {
                backup_toml.push_str(", ");
            }
            backup_toml.push_str(&format!("\"{}\"", f));
        }
        backup_toml.push_str("]\n");
    }

    let mut plan_toml = String::new();
    plan_toml.push_str("\n[plan]\n");
    plan_toml.push_str(&format!("name = \"{}\"\n", plan.metadata.name));
    if let Some(ref v) = plan.metadata.version
        && !v.is_empty()
    {
        plan_toml.push_str(&format!("version = \"{}\"\n", v));
    }
    plan_toml.push_str(&format!("release = {}\n", plan.metadata.release));
    if plan.metadata.epoch > 0 {
        plan_toml.push_str(&format!("epoch = {}\n", plan.metadata.epoch));
    }
    plan_toml.push_str(&format!("arch = \"{}\"\n", plan.metadata.arch));

    format!(
        r#"[part]
name = "{name}"
build_date = "{build_date}"
packager = "wright {wright_version}"
{runtime_deps}{relations}{backup}{plan}{provenance}
"#,
        name = manifest.metadata.name,
        build_date = build_date,
        wright_version = env!("CARGO_PKG_VERSION"),
        runtime_deps = runtime_deps_toml,
        relations = relations_toml,
        backup = backup_toml,
        plan = plan_toml,
        provenance = generate_provenance_toml(plan),
    )
}

/// Render the `[provenance]` section from the plan-level manifest (ADR-0023).
fn generate_provenance_toml(plan: &PlanManifest) -> String {
    let mut toml = String::from("\n[provenance]\n");
    if let Some(ref sum) = plan.plan_checksum {
        toml.push_str(&format!("plan_checksum = \"{}\"\n", sum));
    }
    if !plan.sources.entries.is_empty() {
        toml.push_str("source_checksums = [\n");
        for source in &plan.sources.entries {
            toml.push_str(&format!(
                "    \"{}\",\n",
                source_provenance_line(source, plan)
            ));
        }
        toml.push_str("]\n");
    }
    toml.push_str(&format!(
        "wright_version = \"{}\"\n",
        env!("CARGO_PKG_VERSION")
    ));
    toml.push_str(&format!(
        "isolation = \"{}\"\n",
        weakest_isolation_level(plan)
    ));
    toml
}

/// One provenance line per source: kind, expanded locator, and the
/// verification the charge step applied to it.
fn source_provenance_line(source: &Source, plan: &PlanManifest) -> String {
    use crate::foundry::variables::process_uri;
    match source {
        Source::Http(http) => format!(
            "http {} sha256={}",
            process_uri(&http.url, plan),
            http.sha256
        ),
        Source::Git(git) => format!(
            "git {} ref={}",
            process_uri(&git.url, plan),
            git.r#ref
                .as_deref()
                .map(|r| process_uri(r, plan))
                .unwrap_or_else(|| "HEAD".to_string())
        ),
        Source::Local(local) => format!("local {}", process_uri(&local.path, plan)),
    }
}

/// The weakest isolation level any pipeline stage declares — the
/// security-relevant fact about the build that produced the part.
fn weakest_isolation_level(plan: &PlanManifest) -> &'static str {
    use crate::isolation::IsolationLevel;
    fn rank(level: IsolationLevel) -> u8 {
        match level {
            IsolationLevel::None => 0,
            IsolationLevel::Relaxed => 1,
            IsolationLevel::Strict => 2,
        }
    }
    plan.pipeline
        .values()
        .filter_map(|stage| stage.isolation.parse::<IsolationLevel>().ok())
        .min_by_key(|level| rank(*level))
        .map(|level| match level {
            IsolationLevel::None => "none",
            IsolationLevel::Relaxed => "relaxed",
            IsolationLevel::Strict => "strict",
        })
        .unwrap_or("strict")
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
fn generate_hooks_toml(scripts: &crate::plan::manifest::DeployScripts) -> String {
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
        WrightError::PartError(format!(
            "{}: failed to read .PARTINFO: {}",
            path.display(),
            e
        ))
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

    let parsed: PartInfoToml = toml::from_str(content).map_err(|e| {
        WrightError::PartError(format!("{}: failed to parse .PARTINFO: {}", source, e))
    })?;

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
    use super::{generate_partinfo, parse_partinfo_str};

    #[test]
    fn create_part_refuses_empty_staging_tree() {
        let manifest = crate::plan::manifest::PlanManifest::parse(
            r#"
name = "empty-demo"
version = "1.0.0"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();

        let staging = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();

        // An empty staging tree must not seal (regression: wright 5.0.2
        // packed metadata-only parts that deployed zero files).
        let err = super::create_part(staging.path(), &manifest, out.path(), None).unwrap_err();
        assert!(err.to_string().contains("contains no files"), "{err}");
        assert!(!out.path().join(manifest.part_filename()).exists());

        // The same tree with payload seals fine.
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "x").unwrap();
        let part = super::create_part(staging.path(), &manifest, out.path(), None).unwrap();
        assert!(part.exists());
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
        let toml_str = r#"
name = "demo"
version = "1.2.3"
release = 1
description = "demo"
license = "MIT"
arch = "x86_64"

[[sources]]
type = "http"
url = "https://example.org/demo-${VERSION}.tar.gz"
sha256 = "abc123"

[[sources]]
type = "git"
url = "https://example.org/demo.git"
ref = "v${VERSION}"

[pipeline.compile]
executor = "shell"
isolation = "none"
script = "true"

[pipeline.staging]
executor = "shell"
isolation = "strict"
script = "true"
"#;
        let mut manifest = crate::plan::manifest::PlanManifest::parse(toml_str).unwrap();
        manifest.plan_checksum = Some("deadbeef".to_string());

        let partinfo = generate_partinfo(&manifest, None);
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
        // Weakest of {none, strict} is none.
        assert_eq!(provenance.isolation, "none");
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
}
