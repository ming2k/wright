use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::builder::orchestrator::BuildOptions;
use crate::cli::package::PackageArgs;
use crate::commands::workflow_run::{drive_command, DriveOptions};
use crate::config::GlobalConfig;
use crate::error::WrightError;
use crate::workflow::builders::build_package_workflow;

pub async fn execute_package(
    args: PackageArgs,
    config: &GlobalConfig,
    db_path: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
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
        nproc_per_isolation: config.build.nproc_per_isolation,
        force: args.force,
        ..Default::default()
    };

    let spec = build_package_workflow(
        Arc::new(config.clone()),
        all_targets,
        options,
        args.force,
        args.print_parts,
    )
    .map_err(|e| anyhow::anyhow!("package workflow: {}", e))?;

    drive_command(
        spec,
        DriveOptions {
            config,
            db_path,
            fresh: false,
            quiet,
        },
    )
    .await
    .map(|_| ())
}
