use tracing::info;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_model::isolation::IsolationLevel;
use wright_part::archive;
use wright_part::fhs;
use wright_plan::manifest::{OutputConfig, PlanManifest, Source};

/// Compatibility sealing entry point that derives isolation from explicit
/// stage declarations. Runtime engine paths should pass their resolved policy
/// to [`create_part_with_isolation`].
pub fn create_part(
    part_dir: &std::path::Path,
    manifest: &PlanManifest,
    output_path: &std::path::Path,
    source_plan: Option<&PlanManifest>,
) -> Result<std::path::PathBuf> {
    let plan = source_plan.unwrap_or(manifest);
    create_part_with_isolation(
        part_dir,
        manifest,
        output_path,
        source_plan,
        weakest_declared_isolation(plan),
    )
}

/// Project plan metadata into the archive format and write one part.
pub fn create_part_with_isolation(
    part_dir: &std::path::Path,
    manifest: &PlanManifest,
    output_path: &std::path::Path,
    source_plan: Option<&PlanManifest>,
    isolation: IsolationLevel,
) -> Result<std::path::PathBuf> {
    let plan = source_plan.unwrap_or(manifest);
    let hooks = manifest
        .deploy_scripts
        .as_ref()
        .map(|hooks| archive::PartHooks {
            pre_install: hooks.pre_install.clone(),
            post_install: hooks.post_install.clone(),
            post_upgrade: hooks.post_upgrade.clone(),
            pre_remove: hooks.pre_remove.clone(),
            post_remove: hooks.post_remove.clone(),
        })
        .unwrap_or_default();
    let spec = archive::PartSpec {
        archive_name: manifest.part_filename(),
        name: manifest.metadata.name.clone(),
        runtime_deps: manifest.runtime_deps.clone(),
        replaces: manifest.relations.replaces.clone(),
        conflicts: manifest.relations.conflicts.clone(),
        backup_files: manifest
            .backup
            .as_ref()
            .map(|backup| backup.files.clone())
            .unwrap_or_default(),
        plan: archive::PlanMetadata {
            name: plan.metadata.name.clone(),
            version: plan.metadata.version.clone().unwrap_or_default(),
            release: plan.metadata.release,
            epoch: plan.metadata.epoch,
            arch: plan.metadata.arch.clone(),
        },
        provenance: archive::Provenance {
            plan_checksum: plan.plan_checksum.clone(),
            source_checksums: plan
                .sources
                .entries
                .iter()
                .map(|source| source_provenance_line(source, plan))
                .collect(),
            wright_version: env!("CARGO_PKG_VERSION").to_string(),
            isolation: isolation.to_string(),
        },
        hooks,
    };
    Ok(archive::write_part(part_dir, &spec, output_path)?)
}

fn source_provenance_line(source: &Source, plan: &PlanManifest) -> String {
    use wright_plan::variables::expand_metadata;

    match source {
        Source::Http(http) => format!(
            "http {} sha256={}",
            expand_metadata(&http.url, plan),
            http.sha256
        ),
        Source::Git(git) => format!(
            "git {} ref={}",
            expand_metadata(&git.url, plan),
            git.r#ref
                .as_deref()
                .map(|reference| expand_metadata(reference, plan))
                .unwrap_or_else(|| "HEAD".to_string())
        ),
        Source::Local(local) => format!("local {}", expand_metadata(&local.path, plan)),
    }
}

fn weakest_declared_isolation(plan: &PlanManifest) -> IsolationLevel {
    plan.pipeline
        .values()
        .filter_map(|stage| stage.isolation.as_deref()?.parse::<IsolationLevel>().ok())
        .min()
        .unwrap_or(IsolationLevel::Strict)
}

/// Seal the staging directories for a plan into `.wright.tar.zst` archives.
pub async fn package_outputs(
    manifest: &PlanManifest,
    config: &GlobalConfig,
    result: &crate::foundry::FoundryResult,
    print_parts: bool,
) -> Result<()> {
    tokio::fs::create_dir_all(&config.general.parts_dir)
        .await
        .map_err(WrightError::IoError)?;
    let output_dir = config.general.parts_dir.clone();

    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => {
            for (sub_name, sub_part) in parts {
                let part_dir = if sub_part.include.is_none() {
                    &result.staging_dir
                } else {
                    result.output_dirs.get(sub_name).ok_or_else(|| {
                        WrightError::ForgeError(format!("missing output dir for '{}'", sub_name))
                    })?
                };
                if !manifest.options.skip_fhs_check {
                    fhs::validate(part_dir, sub_name)?;
                }
                let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                let sub_part_path = create_part_with_isolation(
                    part_dir,
                    &sub_manifest,
                    &output_dir,
                    Some(manifest),
                    result.isolation,
                )?;
                let file_name = sub_part_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                // Rule B: no completion line for the seal step; the next
                // package's "Building …" announces implicit success.
                info!(
                    event = "seal.packed",
                    plan_name = %sub_name,
                    part_path = %sub_part_path.display(),
                    file_name = %file_name,
                    "packed"
                );
                if print_parts {
                    println!("{}", sub_part_path.display());
                }
            }
        }
        _ => {
            if !manifest.options.skip_fhs_check {
                fhs::validate(&result.staging_dir, &manifest.metadata.name)?;
            }
            let part_path = create_part_with_isolation(
                &result.staging_dir,
                manifest,
                &output_dir,
                None,
                result.isolation,
            )?;
            let file_name = part_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            info!(
                event = "seal.packed",
                plan_name = %manifest.metadata.name,
                part_path = %part_path.display(),
                file_name = %file_name,
                "packed"
            );
            if print_parts {
                println!("{}", part_path.display());
            }
        }
    }

    Ok(())
}

/// Seal a plan from its existing staging directories.
///
/// When `force` is true, or when `outputs/` is missing / stale, the staging
/// directory is re-sliced according to the current plan manifest before
/// sealing.  This lets users tweak `[[output]]` patterns and re-seal
/// without running a full rebuild.
pub async fn package_manifest(
    manifest: &PlanManifest,
    config: &GlobalConfig,
    print_parts: bool,
    force: bool,
) -> Result<()> {
    let foundry = crate::foundry::Foundry::new(config.clone());
    let build_root = foundry.build_root(manifest)?;
    let isolation = foundry.effective_isolation(manifest, None)?;
    let default_output_dir = build_root.join("outputs").join("default");

    let need_slice = force
        || !default_output_dir.exists()
        || manifest.outputs.as_ref().is_some_and(|cfg| match cfg {
            OutputConfig::Multi(parts) => parts.iter().any(|(sub_name, sub_part)| {
                sub_part.include.is_some() && !build_root.join("outputs").join(sub_name).exists()
            }),
        });

    let result = if need_slice {
        let mold_result = crate::foundry::mold::Mold::slice(manifest, &build_root).await?;
        crate::foundry::FoundryResult {
            staging_dir: build_root.join("staging"),
            build_root: build_root.clone(),
            logs_dir: build_root.join("logs"),
            output_dirs: mold_result.split_dirs,
            isolation,
        }
    } else {
        let mut output_dirs = std::collections::HashMap::new();
        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            for (sub_name, sub_part) in parts {
                if sub_part.include.is_none() {
                    continue;
                }
                output_dirs.insert(sub_name.clone(), build_root.join("outputs").join(sub_name));
            }
        }
        crate::foundry::FoundryResult {
            staging_dir: build_root.join("staging"),
            build_root: build_root.clone(),
            logs_dir: build_root.join("logs"),
            output_dirs,
            isolation,
        }
    };

    package_outputs(manifest, config, &result, print_parts).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealing_projects_plan_metadata_at_the_engine_boundary() {
        let mut manifest = PlanManifest::parse(
            r#"
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

[pipeline.compile]
executor = "shell"
isolation = "none"
script = "true"
"#,
        )
        .unwrap();
        manifest.plan_checksum = Some("deadbeef".to_string());

        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/bin")).unwrap();
        std::fs::write(staging.path().join("usr/bin/demo"), "payload").unwrap();
        let output = tempfile::tempdir().unwrap();

        let path = create_part(staging.path(), &manifest, output.path(), None).unwrap();
        let info = archive::read_partinfo(&path).unwrap();
        let provenance = info.provenance.unwrap();

        assert_eq!(info.plan.name, "demo");
        assert_eq!(info.plan.version, "1.2.3");
        assert_eq!(provenance.plan_checksum.as_deref(), Some("deadbeef"));
        assert_eq!(
            provenance.source_checksums,
            vec!["http https://example.org/demo-1.2.3.tar.gz sha256=abc123"]
        );
        assert_eq!(provenance.isolation, "none");
        assert!(!staging.path().join(".PARTINFO").exists());
        assert!(!staging.path().join(".FILELIST").exists());
    }
}
