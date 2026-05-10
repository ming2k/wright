use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::part::store::LocalPartStore;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::planning::{
    create_execution_plan, resolve_build_set, resolve_explicit_plan_names, BuildExecutionPlan,
    BuildOptions, DependentsMode, MatchPolicy, ResolveOptions,
};
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::spec::{WorkflowBuilder, WorkflowSpec};
use crate::workflow::steps::{
    BuildOptionsCanonical, BuildPlanInputs, BuildPlanStep, InstallBatchInputs, InstallBatchStep,
    InstallSource, PackagePlanInputs, PackagePlanStep, PlanRef,
};

#[derive(Serialize)]
struct ApplyInputs {
    targets: Vec<String>,
    deps: String,
    rdeps: String,
    match_policies: Vec<String>,
    depth: Option<usize>,
    force: bool,
    root_dir: PathBuf,
    options: BuildOptionsCanonical,
}

fn dep_mode_str(d: DependentsMode) -> &'static str {
    match d {
        DependentsMode::None => "none",
        DependentsMode::Link => "link",
        DependentsMode::Runtime => "runtime",
        DependentsMode::Build => "build",
        DependentsMode::All => "all",
    }
}

fn match_str(m: MatchPolicy) -> &'static str {
    match m {
        MatchPolicy::All => "all",
        MatchPolicy::Missing => "missing",
        MatchPolicy::Outdated => "outdated",
        MatchPolicy::Installed => "installed",
    }
}

/// Construct the `wright apply` workflow:
///
/// For each dependency wave produced by `create_execution_plan`:
/// * `BuildPlanStep`   per task — depends on the previous wave's `InstallBatchStep`
/// * `PackagePlanStep` per plan — depends on its `BuildPlanStep`
/// * one `InstallBatchStep` per wave — depends on every `PackagePlanStep` in the wave
#[allow(clippy::too_many_arguments)]
pub async fn build_apply_workflow(
    config: Arc<GlobalConfig>,
    targets: Vec<String>,
    resolve_opts: ResolveOptions,
    options: BuildOptions,
    root_dir: PathBuf,
    part_store: Arc<LocalPartStore>,
    force_install: bool,
    nodeps: bool,
) -> Result<WorkflowSpec> {
    // Compute the build set from the planning layer; the workflow only builds
    // and installs what the resolver says is needed.
    let build_set: Vec<String> = resolve_build_set(&config, targets.clone(), resolve_opts.clone())
        .await
        .map_err(|e| WorkflowError::Other(format!("resolve_build_set: {}", e)))?;

    let plan_dirs = crate::planning::plan_search_dirs(&config);
    let explicit_plan_names = resolve_explicit_plan_names(&plan_dirs, &targets)
        .map_err(|e| WorkflowError::Other(format!("explicit plan names: {}", e)))?;

    // Compute the dep graph + batches over the build set.
    let plan = create_execution_plan(&config, build_set, &options)
        .map_err(|e| WorkflowError::Other(format!("create_execution_plan: {}", e)))?;

    // Workflow inputs derive from the user's stable intent, not from the
    // resolver's output. Different system state may produce a different
    // step set under the same workflow id; orphan rows are GC'd later.
    let mut sorted_targets = targets;
    sorted_targets.sort();
    sorted_targets.dedup();
    let mut sorted_match: Vec<&str> = resolve_opts
        .match_policies
        .iter()
        .copied()
        .map(match_str)
        .collect();
    sorted_match.sort();
    sorted_match.dedup();

    let inputs = ApplyInputs {
        targets: sorted_targets,
        deps: dep_mode_str(resolve_opts.deps.unwrap_or(DependentsMode::None)).to_string(),
        rdeps: dep_mode_str(resolve_opts.rdeps.unwrap_or(DependentsMode::None)).to_string(),
        match_policies: sorted_match.into_iter().map(String::from).collect(),
        depth: resolve_opts.depth,
        force: force_install,
        root_dir: root_dir.clone(),
        options: BuildOptionsCanonical::from_options(&options),
    };

    let mut wfb = WorkflowBuilder::new("apply", &inputs)?;

    let configure_lock = Arc::new(Mutex::new(()));
    let compile_lock = Arc::new(Mutex::new(()));
    let builder = Arc::new(Builder::new((*config).clone()));

    let mut build_step_ids: HashMap<String, StepId> = HashMap::new();
    let mut pkg_step_ids: HashMap<String, StepId> = HashMap::new();
    let mut prev_install: Option<StepId> = None;

    for batch in plan.batches() {
        // Build steps for this wave; depend on previous wave's install.
        let extra: Vec<StepId> = prev_install.iter().cloned().collect();
        let mut bases_in_batch: Vec<String> = Vec::new();
        for task in batch {
            let plan_path = plan
                .plan_path_for_task(task)
                .ok_or_else(|| WorkflowError::other(format!("no path for task {}", task)))?;
            let base = BuildExecutionPlan::task_base_name(task).to_string();
            let is_bootstrap = task.ends_with(":bootstrap");
            let bootstrap_excluded: Vec<String> = {
                let mut v = plan.bootstrap_excluded_for(task).to_vec();
                v.sort();
                v
            };

            let mut effective = BuildOptionsCanonical::from_options(&options);
            if !is_bootstrap && plan.is_post_bootstrap_full(task) {
                effective.force = true;
            }

            let plan_ref = PlanRef::from_path(plan_path, base.clone())?;
            let inputs = BuildPlanInputs {
                plan: plan_ref,
                is_bootstrap,
                bootstrap_excluded,
                options: effective,
            };

            let mut deps: Vec<StepId> = plan
                .deps_for_task(task)
                .iter()
                .filter(|d| plan.build_set().contains(*d))
                .filter_map(|d| build_step_ids.get(d).cloned())
                .collect();
            for d in &extra {
                deps.push(d.clone());
            }

            let id = wfb.add(BuildPlanStep::new(
                inputs,
                deps,
                config.clone(),
                builder.clone(),
                configure_lock.clone(),
                compile_lock.clone(),
                options.nproc_per_isolation,
                options.verbose,
            ))?;
            build_step_ids.insert(task.clone(), id);
            if !is_bootstrap {
                bases_in_batch.push(base);
            }
        }

        // Package steps — one per distinct base in this wave.
        let mut wave_pkg_ids: Vec<StepId> = Vec::new();
        let mut pkg_to_plan_name: Vec<String> = Vec::new();
        let mut explicit_part_names: Vec<String> = Vec::new();
        let mut all_batch_part_names: Vec<String> = Vec::new();
        let mut bases_seen: HashSet<String> = HashSet::new();
        for base in &bases_in_batch {
            if !bases_seen.insert(base.clone()) {
                continue;
            }
            let plan_path = plan
                .plan_path_for_task(base)
                .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                .ok_or_else(|| WorkflowError::other(format!("no plan path for {}", base)))?;
            let manifest = PlanManifest::from_file(plan_path)
                .map_err(|e| WorkflowError::Other(format!("parse plan {}: {}", base, e)))?;

            let plan_ref = PlanRef::from_path(plan_path, base.clone())?;
            let upstream_build = build_step_ids
                .get(base)
                .or_else(|| build_step_ids.get(&format!("{}:bootstrap", base)))
                .cloned();
            let deps = upstream_build.map(|d| vec![d]).unwrap_or_default();

            let pkg_id = wfb.add(PackagePlanStep::new(
                PackagePlanInputs {
                    plan: plan_ref,
                    force: force_install,
                    out_dir: config.general.parts_dir.clone(),
                },
                deps,
                config.clone(),
            ))?;
            pkg_step_ids.insert(base.clone(), pkg_id.clone());
            wave_pkg_ids.push(pkg_id);
            pkg_to_plan_name.push(base.clone());

            // Track which part names this batch will install — needed for
            // explicit-target classification and reconciliation.
            let part_names = manifest_part_names(&manifest);
            for pn in &part_names {
                all_batch_part_names.push(pn.clone());
                if explicit_plan_names.contains(base) {
                    explicit_part_names.push(pn.clone());
                }
            }
        }

        if wave_pkg_ids.is_empty() {
            // Pure bootstrap wave with no full passes; carry prev_install forward.
            continue;
        }

        // Install step — depends on every package step in this wave.
        let sources: Vec<InstallSource> = wave_pkg_ids
            .iter()
            .map(|s| InstallSource::FromPackage { step: s.clone() })
            .collect();
        let mut explicit_sorted = explicit_part_names;
        explicit_sorted.sort();
        explicit_sorted.dedup();
        let mut plans_to_reconcile = pkg_to_plan_name;
        plans_to_reconcile.sort();
        plans_to_reconcile.dedup();

        let label = format!("install-wave-{}", build_step_ids.len());
        let install_id = wfb.add(InstallBatchStep::new(
            InstallBatchInputs {
                label,
                sources,
                explicit_targets: explicit_sorted,
                root_dir: root_dir.clone(),
                force: force_install,
                nodeps,
                plans_to_reconcile,
            },
            wave_pkg_ids,
            part_store.clone(),
        ))?;
        prev_install = Some(install_id);
    }

    Ok(wfb.build())
}

fn manifest_part_names(manifest: &PlanManifest) -> Vec<String> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![manifest.metadata.name.clone()],
    }
}
