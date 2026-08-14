use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

use crate::error::{Result, WrightError};
use tracing::info;

use tokio::sync::Semaphore;

use crate::config::GlobalConfig;
use crate::foundry::{BuildOptions, Foundry};
use crate::operations::drive::{DriveOptions, drive_batches};
use crate::resolve::{BuildExecutionPlan, BuildPlanOptions, DepDomain, create_execution_plan};
use wright_plan::manifest::PlanManifest;

#[derive(Debug, Clone)]
pub struct BuildRequest {
    pub targets: Vec<String>,
    pub stages: Vec<String>,
    pub force_stages: Vec<String>,
    pub until_stage: Option<String>,
    pub skip_check: bool,
    pub clean: bool,
    pub force: bool,
    pub mvp: bool,
    pub fetch_only: bool,
    pub seal: bool,
    pub checksum: bool,
}

pub async fn execute_build(
    request: BuildRequest,
    config: &GlobalConfig,
    db_path: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let _command_lock = wright_state::lock::acquire_lock(
        &wright_state::lock::lock_dir_from_db(db_path),
        wright_state::lock::LockIdentity::Command("build"),
        wright_state::lock::LockMode::Exclusive,
    )
    .map_err(|e| WrightError::context("failed to acquire build command lock", e))?;

    let mut all_targets = request.targets;
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.map_err(WrightError::IoError)?;
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
        && request.stages.is_empty()
        && request.until_stage.is_none()
        && !request.fetch_only
        && !request.checksum;

    if can_fast_path {
        let target = &all_targets[0];
        let plan_path = std::path::Path::new(target);
        if plan_path.is_dir() {
            let manifest = PlanManifest::from_file(&plan_path.join("plan.toml"))
                .map_err(|e| WrightError::context(format!("read plan {}", target), e))?;
            let foundry = Foundry::new(config.clone());
            let mut extra_env = HashMap::new();
            if request.mvp {
                extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
            }
            foundry
                .build(
                    &manifest,
                    plan_path,
                    std::path::Path::new("/"),
                    BuildOptions {
                        stages: request.stages,
                        force_stage: request.force_stages,
                        until_stage: request.until_stage,
                        fetch_only: request.fetch_only,
                        skip_check: request.skip_check,
                        force: request.force,
                        clean: request.clean,
                        extra_env,
                        verbose: verbose > 0,
                        nproc_per_isolation: config.build.nproc_per_isolation,
                        configure_lock: None,
                        compile_lock: None,
                    },
                )
                .await?;
            if request.seal {
                crate::seal::package_manifest(&manifest, config, true, request.force)
                    .await
                    .map_err(|e| WrightError::context(format!("seal {}", target), e))?;
            }
            return Ok(());
        }
    }

    let options = BuildPlanOptions {
        stages: request.stages,
        force_stage: request.force_stages,
        until_stage: request.until_stage,
        fetch_only: request.fetch_only,
        clean: request.clean,
        force: request.force,
        checksum: request.checksum,
        skip_check: request.skip_check,
        verbose: verbose > 0,
        quiet,
        mvp: request.mvp,
        nproc_per_isolation: config.build.nproc_per_isolation,
    };

    let plan = create_execution_plan(
        config,
        all_targets,
        &options,
        DepDomain::BUILD | DepDomain::LINK,
    )
    .map_err(|e| WrightError::context("create execution plan", e))?;

    let plan = Arc::new(plan);
    let foundry = Arc::new(Foundry::new(config.clone()));
    let resources = crate::resolve::summarize_build_resources(config);
    let configure_lock = Arc::new(Semaphore::new(1));
    let compile_lock = Arc::new(Semaphore::new(resources.total_cpus));

    // First Ctrl-C / SIGTERM reaps the build subprocess tree and flips the
    // cancel flag so the batch loop stops; a second one force-quits.
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    crate::cancellation::spawn_signal_handler(cancel_tx, quiet);

    drive_batches(
        &plan,
        &DriveOptions {
            config,
            db_path,
            quiet,
        },
        resources.concurrent_tasks,
        |task| {
            let plan = Arc::clone(&plan);
            let foundry = Arc::clone(&foundry);
            let options = options.clone();
            let configure_lock = Arc::clone(&configure_lock);
            let compile_lock = Arc::clone(&compile_lock);
            let config = config.clone();

            async move {
                let plan_path = plan
                    .plan_path_for_task(&task)
                    .ok_or_else(|| WrightError::ForgeError(format!("no path for task {}", task)))?;
                let base = BuildExecutionPlan::task_base_name(&task);
                let is_bootstrap = task.ends_with(":bootstrap");
                let bootstrap_excluded = plan.bootstrap_excluded_for(&task).to_vec();

                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::context(format!("read plan {}", base), e))?;

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

                // Bootstrap phase: the foundry's hash-chain checkpoint system
                // handles stage invalidation internally.

                let plan_dir = plan_path
                    .parent()
                    .ok_or_else(|| WrightError::ForgeError("plan path has no parent".into()))?
                    .to_path_buf();

                // Intra-step idempotence: skip when staging/ is already
                // populated AND its output matches the manifest recorded in
                // the checkpoint.  Verifying the manifest is essential — a
                // prior hard crash leaves root-owned partial content in
                // staging/ that a bare existence check would mistake for a
                // complete deliverable.
                let build_root = foundry.build_root(&manifest)?;
                let can_short_circuit = !force
                    && options.stages.is_empty()
                    && options.until_stage.is_none()
                    && !options.fetch_only;
                if can_short_circuit && staging_matches_checkpoint(&build_root, &manifest) {
                    info!(event = "build.short_circuited", plan_name = %base, reason = "staging_verified", "Build short-circuited — staging output verified against checkpoint");
                    return Ok(());
                }

                foundry
                    .build(
                        &manifest,
                        &plan_dir,
                        std::path::Path::new("/"),
                        BuildOptions {
                            stages: options.stages.clone(),
                            force_stage: options.force_stage.clone(),
                            until_stage: options.until_stage.clone(),
                            fetch_only: options.fetch_only,
                            skip_check: options.skip_check,
                            force,
                            clean: options.clean,
                            extra_env,
                            verbose: options.verbose,
                            nproc_per_isolation: config.build.nproc_per_isolation,
                            configure_lock: Some(configure_lock),
                            compile_lock: Some(compile_lock),
                        },
                    )
                    .await
                    .map(|_| ())
                // NB: no `build {base}` context wrap here — batch settlement
                // attaches the task name to the failure report itself.
            }
        },
        cancel_rx,
    )
    .await?;

    // Optionally seal every forged base into a part archive.  Bootstrap
    // tasks are skipped: their staging trees are MVP intermediates, and the
    // post-bootstrap full forge of the same base seals the real output.
    if request.seal {
        let mut sealed = std::collections::HashSet::new();
        for batch in plan.batches() {
            for task in batch {
                if task.ends_with(":bootstrap") {
                    continue;
                }
                let base = BuildExecutionPlan::task_base_name(task).to_string();
                if !sealed.insert(base.clone()) {
                    continue;
                }
                let plan_path = plan
                    .plan_path_for_task(task)
                    .ok_or_else(|| WrightError::ForgeError(format!("no plan path for {}", base)))?;
                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::context(format!("read plan {}", base), e))?;
                crate::seal::package_manifest(&manifest, config, true, options.force)
                    .await
                    .map_err(|e| WrightError::context(format!("seal {}", base), e))?;
            }
        }
    }

    Ok(())
}

/// Verify that `staging/` on disk matches the output manifest stored in the
/// checkpoint.  Returns `false` when there is no checkpoint, no recorded
/// manifest, or the on-disk tree does not match — in every such case the
/// caller must run a real build rather than trust the short-circuit.
fn staging_matches_checkpoint(build_root: &std::path::Path, manifest: &PlanManifest) -> bool {
    use crate::foundry::checkpoint::Checkpoint;
    use crate::foundry::staging_manifest::compute_dir_manifest;

    let staging_dir = build_root.join("staging");
    let checkpoint = match Checkpoint::load(
        build_root.to_path_buf(),
        &manifest.metadata.name,
        manifest.metadata.version.as_deref().unwrap_or(""),
    ) {
        Ok(ck) => ck,
        Err(_) => return false,
    };

    let Some(stored) = checkpoint.output_manifest_hash("staging") else {
        return false;
    };

    match compute_dir_manifest(&staging_dir) {
        Ok(actual) => actual == stored,
        Err(_) => false,
    }
}
