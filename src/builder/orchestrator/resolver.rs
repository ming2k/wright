use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::archive::resolver::LocalResolver;
use crate::config::GlobalConfig;
use crate::error::{Result, WrightError, WrightResultExt};
use crate::plan::manifest::PlanManifest;

pub fn setup_resolver(config: &GlobalConfig) -> Result<LocalResolver> {
    let mut resolver = LocalResolver::new();
    resolver.add_plans_dir(config.general.plans_dir.clone());
    for extra_dir in &config.general.extra_plans_dirs {
        resolver.add_plans_dir(extra_dir.clone());
    }

    // Add current directory to plans search path so we can resolve dependencies
    // in local development trees.
    if let Ok(cwd) = std::env::current_dir() {
        resolver.add_plans_dir(cwd.clone());
        resolver.add_search_dir(cwd);
    }

    // Always include the configured parts directory in the part search path.
    resolver.add_search_dir(config.general.parts_dir.clone());

    Ok(resolver)
}

pub fn resolve_targets(
    targets: &[String],
    all_plans: &HashMap<String, PathBuf>,
    resolver: &LocalResolver,
) -> Result<HashSet<PathBuf>> {
    let mut plans_to_build = HashSet::new();

    for target in targets {
        let clean_target = target.trim();
        if clean_target.is_empty() {
            continue;
        }

        if let Some(path) = all_plans.get(clean_target) {
            plans_to_build.insert(path.clone());
        } else {
            let plan_path = PathBuf::from(clean_target);
            let manifest_path = if plan_path.is_file() {
                plan_path
            } else {
                plan_path.join("plan.toml")
            };

            if manifest_path.exists() {
                plans_to_build.insert(manifest_path);
            } else {
                let mut found = false;
                for plans_dir in &resolver.plans_dirs {
                    let candidate = plans_dir.join(clean_target).join("plan.toml");
                    if candidate.exists() {
                        PlanManifest::from_file(&candidate)
                            .context(format!("failed to parse plan '{}'", clean_target))?;
                        plans_to_build.insert(candidate);
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(WrightError::BuildError(format!(
                        "Target not found: {}",
                        clean_target
                    )));
                }
            }
        }
    }

    Ok(plans_to_build)
}
