use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::builder::orchestrator::{create_execution_plan, BuildExecutionPlan, BuildOptions};
use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::spec::{WorkflowBuilder, WorkflowSpec};
use crate::workflow::steps::{BuildOptionsCanonical, BuildPlanInputs, BuildPlanStep, PlanRef};

#[derive(Serialize)]
struct BuildWorkflowInputs {
    targets: Vec<String>,
    options: BuildOptionsCanonical,
}

/// Build a workflow for `wright build`.
///
/// Steps: one `BuildPlanStep` per task in the dep graph, with edges from
/// the `deps_map` restricted to the build set. No package, no install —
/// `wright build` produces staging dirs only.
pub fn build_build_workflow(
    config: Arc<GlobalConfig>,
    targets: Vec<String>,
    options: BuildOptions,
) -> Result<WorkflowSpec> {
    let plan = create_execution_plan(&config, targets.clone(), &options)
        .map_err(|e| WorkflowError::Other(format!("create_execution_plan: {}", e)))?;

    let mut sorted_targets = targets;
    sorted_targets.sort();
    sorted_targets.dedup();
    let canonical = BuildOptionsCanonical::from_options(&options);

    let mut wfb = WorkflowBuilder::new(
        "build",
        &BuildWorkflowInputs {
            targets: sorted_targets,
            options: canonical,
        },
    )?;

    let configure_lock = Arc::new(Mutex::new(()));
    let compile_lock = Arc::new(Mutex::new(()));
    let builder = Arc::new(Builder::new((*config).clone()));

    add_build_steps(
        &plan,
        &options,
        &mut wfb,
        &config,
        &builder,
        &configure_lock,
        &compile_lock,
        Vec::new(),
    )?;

    Ok(wfb.build())
}

/// Add `BuildPlanStep`s for every task in `plan.build_set()`, returning a
/// map from task name to the resulting `StepId`. Other workflow builders
/// (apply, package) call this to compose the build wave on top of their own
/// upstream/downstream steps.
#[allow(clippy::too_many_arguments)]
pub(super) fn add_build_steps(
    plan: &BuildExecutionPlan,
    options: &BuildOptions,
    wfb: &mut WorkflowBuilder,
    config: &Arc<GlobalConfig>,
    builder: &Arc<Builder>,
    configure_lock: &Arc<Mutex<()>>,
    compile_lock: &Arc<Mutex<()>>,
    extra_deps: Vec<StepId>,
) -> Result<HashMap<String, StepId>> {
    let mut step_ids: HashMap<String, StepId> = HashMap::new();
    for batch in plan.batches() {
        for task in batch {
            let plan_path = plan
                .plan_path_for_task(task)
                .ok_or_else(|| WorkflowError::other(format!("no path for task {}", task)))?;
            let base = BuildExecutionPlan::task_base_name(task);
            let is_bootstrap = task.ends_with(":bootstrap");
            let bootstrap_excluded: Vec<String> = {
                let mut v = plan.bootstrap_excluded_for(task).to_vec();
                v.sort();
                v
            };

            let mut effective = BuildOptionsCanonical::from_options(options);
            if !is_bootstrap && plan.is_post_bootstrap_full(task) {
                // Force a fresh full build after the mvp bootstrap pass; the
                // mvp pass produces stage sentinels we must invalidate.
                effective.force = true;
            }

            let plan_ref = PlanRef::from_path(plan_path, base.to_string())?;
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
                .filter_map(|d| step_ids.get(d).cloned())
                .collect();
            for d in &extra_deps {
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
            step_ids.insert(task.clone(), id);
        }
    }
    Ok(step_ids)
}
