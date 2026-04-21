use crate::builder::orchestrator::setup_resolver;
use crate::config::{AssembliesConfig, GlobalConfig};
use crate::error::Result;
use crate::plan::manifest::PlanManifest;
use std::path::PathBuf;
use tracing::{error, info, warn};

pub async fn execute_lint(targets: Vec<String>, _recursive: bool, config: &GlobalConfig) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let assemblies_cfg = AssembliesConfig::load_all(&config.general.assemblies_dir)?;

    let mut plan_targets = Vec::new();
    let mut assembly_targets = Vec::new();

    if targets.is_empty() {
        plan_targets.extend(all_plans.values().cloned());
        assembly_targets.extend(assemblies_cfg.assemblies.keys().cloned());
    } else {
        for target in targets {
            if let Some(assembly_name) = target.strip_prefix('@') {
                if assemblies_cfg.assemblies.contains_key(assembly_name) {
                    assembly_targets.push(assembly_name.to_string());
                } else {
                    error!("Assembly not found: @{}", assembly_name);
                    return Err(crate::error::WrightError::ValidationError(format!("Assembly not found: @{}", assembly_name)));
                }
            } else if let Some(path) = all_plans.get(&target) {
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

    for name in &assembly_targets {
        let assembly = assemblies_cfg.assemblies.get(name).unwrap();
        let mut assembly_failed = false;
        for plan_name in &assembly.plans {
            if !all_plans.contains_key(plan_name) {
                error!("Assembly [ERR]: @{} references non-existent plan '{}'", name, plan_name);
                assembly_failed = true;
            }
        }
        for include_name in &assembly.includes {
            if !assemblies_cfg.assemblies.contains_key(include_name) {
                error!("Assembly [ERR]: @{} includes non-existent assembly '@{}'", name, include_name);
                assembly_failed = true;
            }
        }
        if assembly_failed {
            failed += 1;
        } else {
            info!("Assembly [OK]: @{}", name);
        }
    }

    if failed > 0 {
        return Err(crate::error::WrightError::ValidationError(format!("Lint failed for {} target(s)", failed)));
    }
    info!("Lint passed: {} plans, {} assemblies.", plan_targets.len(), assembly_targets.len());
    Ok(())
}
