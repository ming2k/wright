use tracing::info;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::fhs;
use crate::plan::manifest::{OutputConfig, PlanManifest};

/// Seal the staging directories for a plan into `.wright.tar.zst` archives.
pub async fn package_outputs(
    manifest: &PlanManifest,
    config: &GlobalConfig,
    result: &crate::forge::ForgeResult,
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
                    &result.output_dir
                } else {
                    result.split_part_dirs.get(sub_name).ok_or_else(|| {
                        WrightError::ForgeError(format!("missing output dir for '{}'", sub_name))
                    })?
                };
                if !manifest.options.skip_fhs_check {
                    fhs::validate(part_dir, sub_name)?;
                }
                let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                let sub_part_path =
                    archive::create_part(part_dir, &sub_manifest, &output_dir, Some(manifest))?;
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
                fhs::validate(&result.output_dir, &manifest.metadata.name)?;
            }
            let part_path = archive::create_part(&result.output_dir, manifest, &output_dir, None)?;
            let file_name = part_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
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
/// without running a full reforge.
pub async fn package_manifest(
    manifest: &PlanManifest,
    config: &GlobalConfig,
    print_parts: bool,
    force: bool,
) -> Result<()> {
    let forger = crate::forge::Forger::new(config.clone());
    let build_root = forger.build_root(manifest)?;
    let output_dir = build_root.join("outputs").join("default");

    let need_slice = force
        || !output_dir.exists()
        || manifest.outputs.as_ref().is_some_and(|cfg| match cfg {
            OutputConfig::Multi(parts) => parts.iter().any(|(sub_name, sub_part)| {
                sub_part.include.is_some() && !build_root.join("outputs").join(sub_name).exists()
            }),
        });

    let result = if need_slice {
        forger.slice_outputs(manifest, &build_root).await?
    } else {
        let mut split_part_dirs = std::collections::HashMap::new();
        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            for (sub_name, sub_part) in parts {
                if sub_part.include.is_none() {
                    continue;
                }
                split_part_dirs.insert(sub_name.clone(), build_root.join("outputs").join(sub_name));
            }
        }
        crate::forge::ForgeResult {
            output_dir,
            work_dir: build_root.clone(),
            logs_dir: build_root.join("logs"),
            split_part_dirs,
        }
    };

    package_outputs(manifest, config, &result, print_parts).await
}
