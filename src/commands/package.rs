use anyhow::{Context, Result};
use std::io::BufRead;

use crate::builder::orchestrator;
use crate::cli::package::PackageArgs;
use crate::config::GlobalConfig;
use crate::error::WrightError;
use crate::plan::manifest::PlanManifest;

pub async fn execute_package(
    args: PackageArgs,
    config: &GlobalConfig,
    _verbose: u8,
    quiet: bool,
) -> Result<()> {
    let _command_lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(&config.general.db_path),
        crate::util::lock::LockIdentity::Command("package"),
        crate::util::lock::LockMode::Exclusive,
    )
    .context("failed to acquire package command lock")?;

    let mut all_targets = args.targets;
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.context("failed to read target from stdin")?;
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                all_targets.push(trimmed);
            }
        }
    }

    if all_targets.is_empty() {
        return Err(WrightError::BuildError(
            "No targets specified to package.".to_string(),
        )
        .into());
    }

    let resolver = orchestrator::setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let plan_paths = orchestrator::resolve_targets(&all_targets, &all_plans, &resolver)?;

    if plan_paths.is_empty() {
        return Err(WrightError::BuildError(
            "No targets found matching the requested names.".to_string(),
        )
        .into());
    }

    for plan_path in plan_paths {
        let manifest = PlanManifest::from_file(&plan_path)
            .with_context(|| format!("failed to read plan: {}", plan_path.display()))?;

        if !quiet {
            tracing::info!("Packaging {}...", manifest.plan.name);
        }

        let parts_dir = if config.general.parts_dir.exists()
            || tokio::fs::create_dir_all(&config.general.parts_dir)
                .await
                .is_ok()
        {
            config.general.parts_dir.clone()
        } else {
            std::env::current_dir().map_err(WrightError::IoError)?
        };

        if !args.force {
            let all_exist = match manifest.outputs {
                Some(crate::plan::manifest::OutputConfig::Multi(ref parts)) => {
                    parts.iter().all(|(sub_name, sub_part)| {
                        let sub_manifest = sub_part.to_manifest(sub_name, &manifest);
                        parts_dir.join(sub_manifest.part_filename()).exists()
                    })
                }
                _ => parts_dir.join(manifest.part_filename()).exists(),
            };
            if all_exist {
                if !quiet {
                    tracing::info!(
                        "{}",
                        crate::builder::logging::plan_skipped_existing(&manifest.plan.name
                        )
                    );
                }
                continue;
            }
        }

        orchestrator::package_manifest(&manifest, config, args.print_parts, args.force)
            .await
            .with_context(|| format!("failed to package {}", manifest.plan.name))?;
    }

    Ok(())
}
