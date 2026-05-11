use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::Path;

use crate::cli::package::PackageArgs;
use crate::config::GlobalConfig;
use crate::error::WrightError;
use crate::plan::manifest::PlanManifest;
use crate::planning::{package_manifest, plan_search_dirs, resolve_targets};

pub async fn execute_package(
    args: PackageArgs,
    config: &GlobalConfig,
    db_path: &Path,
    _verbose: u8,
    _quiet: bool,
) -> Result<()> {
    let mut command_config = config.clone();
    if let Some(out_dir) = args.out_dir {
        command_config.general.parts_dir =
            normalize_out_dir(out_dir).context("failed to resolve package output directory")?;
    }

    let _command_lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(db_path),
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
        return Err(WrightError::BuildError("No targets specified to package.".to_string()).into());
    }

    let plan_dirs = plan_search_dirs(&command_config);
    let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
    let plans_to_build = resolve_targets(&all_targets, &index, &plan_dirs)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::BuildError("No targets found to package.".to_string()).into());
    }

    for plan_path in plans_to_build {
        let manifest = PlanManifest::from_file(&plan_path)
            .with_context(|| format!("read plan {}", plan_path.display()))?;
        package_manifest(&manifest, &command_config, args.print_parts, args.force)
            .await
            .with_context(|| format!("package {}", manifest.metadata.name))?;
    }

    Ok(())
}

fn normalize_out_dir(path: std::path::PathBuf) -> Result<std::path::PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
