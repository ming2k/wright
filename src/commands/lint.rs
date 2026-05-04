use crate::builder::orchestrator::{self, setup_resolver};
use crate::config::GlobalConfig;
use crate::error::Result;
use crate::plan::manifest::PlanManifest;
use std::path::PathBuf;
use tracing::{error, info, warn};

pub async fn execute_lint(
    targets: Vec<String>,
    _recursive: bool,
    config: &GlobalConfig,
) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;

    let mut plan_targets = Vec::new();

    if targets.is_empty() {
        plan_targets.extend(all_plans.values().cloned());
    } else {
        for target in targets {
            if let Some(path) = all_plans.get(&target) {
                plan_targets.push(path.clone());
            } else {
                let path = PathBuf::from(&target);
                if path.is_file() {
                    plan_targets.push(path);
                } else {
                    warn!("Target not found: {}", target);
                }
            }
        }
    }

    let mut failed = 0;

    // 1. Lint individual plan manifests
    for path in &plan_targets {
        match PlanManifest::from_file(path) {
            Ok(m) => {
                info!("Plan [OK]: {} ({})", m.plan.name, path.display());
            }
            Err(e) => {
                error!("Plan [ERR]: {} - {}", path.display(), e);
                failed += 1;
            }
        }
    }

    // 2. Lint dependency graph when targets are specified
    let plan_names: Vec<String> = plan_targets
        .iter()
        .filter_map(|p| {
            PlanManifest::from_file(p)
                .ok()
                .map(|m| m.plan.name)
        })
        .collect();

    if !plan_names.is_empty() {
        if let Err(e) = orchestrator::lint_dependency_graph_for_targets(config, &plan_names) {
            error!("Dependency graph analysis failed: {}", e);
            failed += 1;
        }
    }

    if failed > 0 {
        return Err(crate::error::WrightError::ValidationError(format!(
            "Lint failed for {} target(s)",
            failed
        )));
    }
    info!("Lint passed: {} plans.", plan_targets.len());
    Ok(())
}
