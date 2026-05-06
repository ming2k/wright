use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::builder::orchestrator::BuildOptions;
use crate::cli::build::BuildArgs;
use crate::commands::workflow_run::{drive_command, DriveOptions};
use crate::config::GlobalConfig;
use crate::workflow::builders::build_build_workflow;

pub async fn execute_build(
    args: BuildArgs,
    config: &GlobalConfig,
    db_path: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let _command_lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(db_path),
        crate::util::lock::LockIdentity::Command("build"),
        crate::util::lock::LockMode::Exclusive,
    )
    .context("failed to acquire build command lock")?;

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

    let options = BuildOptions {
        stages: args.stage,
        until_stage: args.until_stage,
        fetch_only: args.fetch,
        clean: args.clean,
        force: args.force,
        checksum: args.checksum,
        skip_check: args.skip_check,
        verbose: verbose > 0,
        quiet,
        mvp: args.mvp,
        nproc_per_isolation: config.build.nproc_per_isolation,
    };

    let spec = build_build_workflow(Arc::new(config.clone()), all_targets, options)
        .map_err(|e| anyhow::anyhow!("build workflow: {}", e))?;

    drive_command(
        spec,
        DriveOptions {
            config,
            db_path,
            fresh: args.fresh,
            quiet,
        },
    )
    .await
    .map(|_| ())
}
