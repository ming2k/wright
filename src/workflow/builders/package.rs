use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::builder::orchestrator::{create_execution_plan, BuildExecutionPlan, BuildOptions};
use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::plan::manifest::PlanManifest;
use crate::workflow::errors::{Result, WorkflowError};
use crate::workflow::id::StepId;
use crate::workflow::spec::{WorkflowBuilder, WorkflowSpec};
use crate::workflow::steps::{
    BuildOptionsCanonical, PackagePlanInputs, PackagePlanStep, PlanRef,
};

use super::build::add_build_steps;

#[derive(Serialize)]
struct PackageWorkflowInputs {
    targets: Vec<String>,
    options: BuildOptionsCanonical,
    force: bool,
}

/// Build a workflow for `wright package`: build → package per plan.
pub fn build_package_workflow(
    config: Arc<GlobalConfig>,
    targets: Vec<String>,
    options: BuildOptions,
    force_repack: bool,
    print_parts: bool,
) -> Result<WorkflowSpec> {
    let plan = create_execution_plan(&config, targets.clone(), &options).map_err(|e| {
        WorkflowError::Other(format!("create_execution_plan: {}", e))
    })?;

    let mut sorted_targets = targets;
    sorted_targets.sort();
    sorted_targets.dedup();

    let mut wfb = WorkflowBuilder::new(
        "package",
        &PackageWorkflowInputs {
            targets: sorted_targets,
            options: BuildOptionsCanonical::from_options(&options),
            force: force_repack,
        },
    )?;

    let configure_lock = Arc::new(Mutex::new(()));
    let compile_lock = Arc::new(Mutex::new(()));
    let builder = Arc::new(Builder::new((*config).clone()));

    let build_ids = add_build_steps(
        &plan,
        &options,
        &mut wfb,
        &config,
        &builder,
        &configure_lock,
        &compile_lock,
        Vec::new(),
    )?;

    add_package_steps(&plan, &mut wfb, &config, force_repack, print_parts, &build_ids)?;

    Ok(wfb.build())
}

/// Add a `PackagePlanStep` for every distinct base plan in `plan.build_set()`.
/// Returns a map from plan name to its package step id.
pub(super) fn add_package_steps(
    plan: &BuildExecutionPlan,
    wfb: &mut WorkflowBuilder,
    config: &Arc<GlobalConfig>,
    force_repack: bool,
    print_parts: bool,
    build_ids: &HashMap<String, StepId>,
) -> Result<HashMap<String, StepId>> {
    let mut pkg_ids: HashMap<String, StepId> = HashMap::new();
    let mut seen_bases: std::collections::HashSet<String> = std::collections::HashSet::new();
    for task in plan.build_set() {
        let base = BuildExecutionPlan::task_base_name(task).to_string();
        if !seen_bases.insert(base.clone()) {
            continue;
        }
        let plan_path = plan
            .plan_path_for_task(&base)
            .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
            .ok_or_else(|| WorkflowError::other(format!("no plan path for {}", base)))?;
        let _manifest = PlanManifest::from_file(plan_path)
            .map_err(|e| WorkflowError::Other(format!("parse plan {}: {}", base, e)))?;
        let plan_ref = PlanRef::from_path(plan_path, base.clone())?;

        // Package depends on the full-pass build step; if only the bootstrap
        // pass is in the workflow (rare), package depends on that instead.
        let upstream = build_ids
            .get(&base)
            .or_else(|| build_ids.get(&format!("{}:bootstrap", base)))
            .cloned();
        let deps = upstream.map(|d| vec![d]).unwrap_or_default();

        let id = wfb.add(
            PackagePlanStep::new(
                PackagePlanInputs {
                    plan: plan_ref,
                    force: force_repack,
                },
                deps,
                config.clone(),
            )
            .with_print_parts(print_parts),
        )?;
        pkg_ids.insert(base, id);
    }
    Ok(pkg_ids)
}
