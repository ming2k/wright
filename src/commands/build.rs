use anyhow::{Context, Result};
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::builder::Builder;
use crate::cli::build::BuildArgs;
use crate::config::GlobalConfig;
use crate::operations::drive::{drive_command, DriveOptions};
use crate::plan::manifest::PlanManifest;
use crate::planning::BuildOptions;
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

    if all_targets.is_empty() {
        anyhow::bail!("no targets specified");
    }

    // Fast path: single target with no dep resolution needed.
    // Bypass workflow construction and drive for the common case of
    // `wright build myplan`, reducing overhead by ~10-20ms.
    let can_fast_path = all_targets.len() == 1
        && !all_targets[0].starts_with('@')
        && args.stage.is_empty()
        && args.until_stage.is_none()
        && !args.fetch
        && !args.checksum
        && !args.invalidate;

    if can_fast_path {
        let target = &all_targets[0];
        let plan_path = std::path::Path::new(target);
        if plan_path.is_dir() {
            let manifest = PlanManifest::from_file(&plan_path.join("plan.toml"))
                .with_context(|| format!("read plan {}", target))?;
            let builder = Builder::new(config.clone());
            let mut extra_env = std::collections::HashMap::new();
            if args.mvp {
                extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
            }
            builder
                .build(
                    &manifest,
                    plan_path,
                    std::path::Path::new("/"),
                    &args.stage,
                    &args.force_stage,
                    args.until_stage.as_deref(),
                    args.fetch,
                    args.skip_check,
                    args.rebuild,
                    &extra_env,
                    verbose > 0,
                    config.build.nproc_per_isolation,
                    None,
                    None,
                    None,
                )
                .await?;
            return Ok(());
        }
    }

    let options = BuildOptions {
        stages: args.stage,
        force_stage: args.force_stage,
        until_stage: args.until_stage,
        fetch_only: args.fetch,
        clean: args.clean,
        force: args.rebuild,
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
            invalidate: args.invalidate,
            quiet,
        },
    )
    .await
    .map(|_| ())
}
