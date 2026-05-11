use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Result, WrightError};
use tokio::sync::Mutex;
use tracing::info;

use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::group;
use crate::part::store::LocalPartStore;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::planning::{
    create_execution_plan, resolve_build_set, resolve_explicit_plan_names, BuildExecutionPlan,
    BuildOptions, DependentsMode, MatchPolicy, ResolveOptions,
};

pub struct ApplyRequest<'a> {
    pub targets: Vec<String>,
    pub deps: Option<DependentsMode>,
    pub rdeps: Option<DependentsMode>,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub force: bool,
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub root_dir: &'a Path,
    pub verbose: u8,
    pub quiet: bool,
    pub part_store: &'a LocalPartStore,
}

pub async fn execute_apply(request: ApplyRequest<'_>) -> Result<()> {
    let ApplyRequest {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        force,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
    } = request;

    if targets.is_empty() {
        return Err(WrightError::BuildError(
            "no targets specified (pass plan names, group names prefixed with '@', or paths as arguments or via stdin)".into()
        ));
    }

    let groups_dirs: Vec<PathBuf> = vec![config.general.groups_dir.clone()];
    let (targets, group_assumes, _group_config) =
        group::expand_group_references(targets, &groups_dirs)?;

    if targets.is_empty() {
        return Err(WrightError::BuildError("no plans to build after expanding groups".into()));
    }

    register_group_assumptions(db_path, &group_assumes).await?;

    let resolve_opts = ResolveOptions {
        deps: Some(deps.unwrap_or(DependentsMode::All)),
        rdeps,
        match_policies: if match_policies.is_empty() {
            vec![MatchPolicy::Outdated]
        } else {
            match_policies
        },
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        preserve_targets: force,
    };

    let build_opts = BuildOptions {
        clean: force,
        force,
        verbose: verbose > 0,
        quiet,
        nproc_per_isolation: config.build.nproc_per_isolation,
        ..Default::default()
    };

    let build_set: Vec<String> =
        resolve_build_set(config, targets.clone(), resolve_opts.clone())
            .await
            .map_err(|e| WrightError::BuildError(format!("resolve_build_set: {}", e)))?;

    let plan_dirs = crate::planning::plan_search_dirs(config);
    let explicit_plan_names = resolve_explicit_plan_names(&plan_dirs, &targets)
        .map_err(|e| WrightError::BuildError(format!("explicit plan names: {}", e)))?;

    let plan = create_execution_plan(config, build_set, &build_opts)
        .map_err(|e| WrightError::BuildError(format!("create_execution_plan: {}", e)))?;

    let plan = Arc::new(plan);
    let builder = Arc::new(Builder::new(config.clone()));
    let configure_lock = Arc::new(Mutex::new(()));
    let compile_lock = Arc::new(Mutex::new(()));
    let _resources = crate::planning::summarize_build_resources(config);

    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    let total_batches = plan.batches().len();

    for (batch_idx, batch) in plan.batches().iter().enumerate() {
        if !quiet {
            info!(
                "apply batch {}/{}: {} task(s)",
                batch_idx + 1,
                total_batches,
                batch.len()
            );
        }

        // 1. Build all tasks in this batch in parallel.
        let mut build_handles = Vec::new();
        for task in batch {
            let plan = Arc::clone(&plan);
            let builder = Arc::clone(&builder);
            let build_opts = build_opts.clone();
            let configure_lock = Arc::clone(&configure_lock);
            let compile_lock = Arc::clone(&compile_lock);
            let config = config.clone();
            let task = task.clone();
            let task_for_handle = task.clone();

            let handle = tokio::spawn(async move {
                let plan_path = plan
                    .plan_path_for_task(&task_for_handle)
                    .ok_or_else(|| WrightError::BuildError(format!("no path for task {}", task_for_handle)))?;
                let base = BuildExecutionPlan::task_base_name(&task_for_handle);
                let is_bootstrap = task_for_handle.ends_with(":bootstrap");
                let bootstrap_excluded = plan.bootstrap_excluded_for(&task_for_handle).to_vec();

                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::BuildError(format!("read plan {}: {}", base, e)))?;

                let mut extra_env = HashMap::new();
                if is_bootstrap || build_opts.mvp {
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

                let force = if !is_bootstrap && plan.is_post_bootstrap_full(&task_for_handle) {
                    let build_root = builder.build_root(&manifest)?;
                    let work_dir = build_root.join("work");
                    let ck = crate::builder::checkpoint::StageCheckpoint::new(
                        work_dir,
                        Some("mvp".to_string()),
                    );
                    ck.invalidate_all();
                    true
                } else {
                    build_opts.force
                };

                if is_bootstrap {
                    let build_root = builder.build_root(&manifest)?;
                    let work_dir = build_root.join("work");
                    let ck = crate::builder::checkpoint::StageCheckpoint::new(
                        work_dir,
                        Some("mvp".to_string()),
                    );
                    ck.invalidate_from("staging");
                }

                let plan_dir = plan_path
                    .parent()
                    .ok_or_else(|| WrightError::BuildError("plan path has no parent".into()))?
                    .to_path_buf();

                builder
                    .build(
                        &manifest,
                        &plan_dir,
                        std::path::Path::new("/"),
                        &build_opts.stages,
                        &build_opts.force_stage,
                        build_opts.until_stage.as_deref(),
                        build_opts.fetch_only,
                        build_opts.skip_check,
                        force,
                        &extra_env,
                        build_opts.verbose,
                        config.build.nproc_per_isolation,
                        Some(configure_lock),
                        Some(compile_lock),
                        None,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| WrightError::BuildError(format!("build {}: {}", base, e)))
            });
            build_handles.push((task.clone(), handle));
        }

        for (task, handle) in build_handles {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(WrightError::BuildError(format!("task '{}' failed: {}", task, e))),
                Err(e) => return Err(WrightError::BuildError(format!("task '{}' panicked: {}", task, e))),
            }
        }

        // 2. Package distinct non-bootstrap bases in this batch.
        let mut bases_in_batch: Vec<String> = Vec::new();
        let mut bases_seen: HashSet<String> = HashSet::new();
        for task in batch {
            let base = BuildExecutionPlan::task_base_name(task).to_string();
            if task.ends_with(":bootstrap") || !bases_seen.insert(base.clone()) {
                continue;
            }
            bases_in_batch.push(base);
        }

        for base in &bases_in_batch {
            let plan_path = plan
                .plan_path_for_task(base)
                .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                    .ok_or_else(|| WrightError::BuildError(format!("no plan path for {}", base)))?;
            let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::BuildError(format!("parse plan {}: {}", base, e)))?;

            crate::planning::package_manifest(&manifest, config, false, force)
                .await
                .map_err(|e| WrightError::BuildError(format!("package {}: {}", base, e)))?;
        }

        // 3. Install this wave.
        if !bases_in_batch.is_empty() {
            let mut archive_paths: Vec<PathBuf> = Vec::new();
            let mut explicit: HashSet<String> = HashSet::new();

            for base in &bases_in_batch {
                let plan_path = plan
                    .plan_path_for_task(base)
                    .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                .ok_or_else(|| WrightError::BuildError(format!("no plan path for {}", base)))?;
                let manifest = PlanManifest::from_file(plan_path)
                .map_err(|e| WrightError::BuildError(format!("parse plan {}: {}", base, e)))?;

                let part_names = manifest_part_names(&manifest);
                for pn in &part_names {
                    let resolved = part_store
                        .resolve(pn)
                        .await
                        .map_err(|e| WrightError::PartError(format!("resolve part {} after packaging: {}", pn, e)))?
                        .ok_or_else(|| WrightError::PartNotFound(format!("part {} not found after packaging", pn)))?;
                    archive_paths.push(resolved.path);
                    if explicit_plan_names.contains(base) {
                        explicit.insert(pn.clone());
                    }
                }
            }

            if !archive_paths.is_empty() {
                crate::transaction::install_parts_with_explicit_targets(
                    &db,
                    &archive_paths,
                    &explicit,
                    root_dir,
                    part_store,
                    force,
                    false,
                )
                .await
                .map_err(|e| WrightError::InstallError(format!("install batch: {}", e)))?;
            }
        }
    }

    Ok(())
}

async fn register_group_assumptions(
    db_path: &Path,
    assumptions: &[group::GroupAssume],
) -> Result<()> {
    if assumptions.is_empty() {
        return Ok(());
    }

    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to open database for group assumptions: {}", e)))?;
    for assume in assumptions {
        db.assume_part(&assume.name, &assume.version)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to assume {}: {}", assume.name, e)))?;
    }
    Ok(())
}

fn manifest_part_names(manifest: &PlanManifest) -> Vec<String> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![manifest.metadata.name.clone()],
    }
}
