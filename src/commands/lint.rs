use std::path::PathBuf;
use tracing::{info, error, warn};
use crate::error::Result;
use crate::plan::manifest::PlanManifest;
use crate::builder::orchestrator::setup_resolver;
use crate::config::GlobalConfig;

pub fn execute_lint(
    targets: Vec<String>,
    _recursive: bool,
    config: &GlobalConfig,
) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;

    let mut lint_targets = Vec::new();
    if targets.is_empty() {
        // Default to all plans in plans_dir
        lint_targets.extend(all_plans.values().cloned());
    } else {
        for target in targets {
            if let Some(path) = all_plans.get(&target) {
                lint_targets.push(path.clone());
            } else {
                let path = PathBuf::from(&target);
                if path.is_file() {
                    lint_targets.push(path);
                } else {
                    warn!("Target not found: {}", target);
                }
            }
        }
    }

    let mut failed = 0;
    for path in &lint_targets {
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

    if failed > 0 {
        return Err(crate::error::WrightError::ValidationError(format!(
            "Lint failed for {} plan(s)",
            failed
        )));
    }

    info!("All {} plans passed lint.", lint_targets.len());
    Ok(())
}
