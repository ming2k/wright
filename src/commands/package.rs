use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::cli::package::PackageArgs;
use crate::config::GlobalConfig;
use crate::error::WrightError;
use crate::operations::drive::{drive_command, DriveOptions};
use crate::planning::BuildOptions;
use crate::workflow::builders::build_package_workflow;

pub async fn execute_package(
    args: PackageArgs,
    config: &GlobalConfig,
    db_path: &Path,
    verbose: u8,
    quiet: bool,
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

    let options = BuildOptions {
        verbose: verbose > 0,
        quiet,
        nproc_per_isolation: command_config.build.nproc_per_isolation,
        force: args.force,
        ..Default::default()
    };

    let spec = build_package_workflow(
        Arc::new(command_config.clone()),
        all_targets,
        options,
        args.force,
        args.print_parts,
    )
    .map_err(|e| anyhow::anyhow!("package workflow: {}", e))?;

    drive_command(
        spec,
        DriveOptions {
            config: &command_config,
            db_path,
            invalidate: false,
            quiet,
        },
    )
    .await
    .map(|_| ())
}

fn normalize_out_dir(path: std::path::PathBuf) -> Result<std::path::PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
