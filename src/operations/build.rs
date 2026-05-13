use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::error::{Result, WrightError};

use tokio::sync::Mutex;

use crate::cli::build::BuildArgs;
use crate::config::GlobalConfig;
use crate::forge::Forger;
use crate::operations::drive::{DriveOptions, drive_batches};
use crate::plan::manifest::PlanManifest;
use crate::resolve::{DepDomain, ForgeExecutionPlan, ForgeOptions, create_execution_plan};
use crate::util::progress::{self, ProgressBarGuard};

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
    .map_err(|e| WrightError::LockError(format!("failed to acquire build command lock: {}", e)))?;

    let flow_spinner = progress::new_build_flow_spinner();
    let _flow_guard = ProgressBarGuard(flow_spinner.clone());

    let mut all_targets = args.targets;
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.map_err(|e| WrightError::IoError(e))?;
            let trimmed = line.trim().to_string();
            if !trimmed.is_empty() {
                all_targets.push(trimmed);
            }
        }
    }

    if all_targets.is_empty() {
        return Err(WrightError::ForgeError("no targets specified".into()));
    }

    // Fast path: single target with no dep resolution needed.
    let can_fast_path = all_targets.len() == 1
        && !all_targets[0].starts_with('@')
        && args.stage.is_empty()
        && args.until_stage.is_none()
        && !args.fetch
        && !args.checksum;

    if can_fast_path {
        let target = &all_targets[0];
        let plan_path = std::path::Path::new(target);
        if plan_path.is_dir() {
            let manifest = PlanManifest::from_file(&plan_path.join("plan.toml"))
                .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", target, e)))?;
            flow_spinner.set_message("building".to_string());
            let forger = Forger::new(config.clone());
            let mut extra_env = HashMap::new();
            if args.mvp {
                extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
            }
            forger
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
                )
                .await?;
            return Ok(());
        }
    }

    let options = ForgeOptions {
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

    flow_spinner.set_message("resolving build graph".to_string());

    let plan = create_execution_plan(
        &config,
        all_targets,
        &options,
        DepDomain::BUILD | DepDomain::LINK,
    )
    .map_err(|e| WrightError::ForgeError(format!("create execution plan: {}", e)))?;

    let plan = Arc::new(plan);
    let forger = Arc::new(Forger::new(config.clone()));
    let configure_lock = Arc::new(Mutex::new(()));
    let compile_lock = Arc::new(Mutex::new(()));
    let resources = crate::resolve::summarize_forge_resources(config);

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = cancel_tx.send(true);
    });

    drive_batches(
        &plan,
        &DriveOptions {
            config,
            db_path,
            quiet,
            flow_progress: Some(flow_spinner.clone()),
        },
        resources.concurrent_tasks,
        |task| {
            let plan = Arc::clone(&plan);
            let forger = Arc::clone(&forger);
            let options = options.clone();
            let configure_lock = Arc::clone(&configure_lock);
            let compile_lock = Arc::clone(&compile_lock);
            let config = config.clone();

            async move {
                let plan_path = plan
                    .plan_path_for_task(&task)
                    .ok_or_else(|| WrightError::ForgeError(format!("no path for task {}", task)))?;
                let base = ForgeExecutionPlan::task_base_name(&task);
                let is_bootstrap = task.ends_with(":bootstrap");
                let bootstrap_excluded = plan.bootstrap_excluded_for(&task).to_vec();

                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", base, e)))?;

                let mut extra_env = HashMap::new();
                if is_bootstrap || options.mvp {
                    extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
                    for dep in &bootstrap_excluded {
                        let key = format!(
                            "WRIGHT_BOOTSTRAP_WITHOUT_{}",
                            dep.to_uppercase().replace('-', "_")
                        );
                        extra_env.insert(key, "1".to_string());
                    }
                } else {
                    extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "full".to_string());
                }

                // Post-bootstrap full forges need to invalidate mvp checkpoints
                let force = if !is_bootstrap && plan.is_post_bootstrap_full(&task) {
                    true
                } else {
                    options.force
                };

                // Bootstrap phase: the forger's hash-chain checkpoint system
                // handles stage invalidation internally.

                let plan_dir = plan_path
                    .parent()
                    .ok_or_else(|| WrightError::ForgeError("plan path has no parent".into()))?
                    .to_path_buf();

                // Intra-step idempotence: skip when staging/ is already populated.
                let build_root = forger.build_root(&manifest)?;
                let can_short_circuit = !force
                    && options.stages.is_empty()
                    && options.until_stage.is_none()
                    && !options.fetch_only;
                if can_short_circuit && staging_is_populated(&build_root) {
                    tracing::info!("{} already built; reusing populated staging/", base);
                    return Ok(());
                }

                forger
                    .build(
                        &manifest,
                        &plan_dir,
                        std::path::Path::new("/"),
                        &options.stages,
                        &options.force_stage,
                        options.until_stage.as_deref(),
                        options.fetch_only,
                        options.skip_check,
                        force,
                        &extra_env,
                        options.verbose,
                        config.build.nproc_per_isolation,
                        Some(configure_lock),
                        Some(compile_lock),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| WrightError::ForgeError(format!("build {}: {}", base, e)))
            }
        },
        cancel_rx,
    )
    .await
}

fn staging_is_populated(build_root: &std::path::Path) -> bool {
    dir_is_populated(&build_root.join("staging"))
}

fn dir_is_populated(dir: &std::path::Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return true;
            }
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) && dir_is_populated(&p) {
                return true;
            }
        }
    }
    false
}
