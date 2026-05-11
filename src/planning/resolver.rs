use std::collections::HashSet;
use std::path::PathBuf;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError, WrightResultExt};
use crate::part::store::LocalPartStore;
use crate::plan::discovery::PlanIndex;
use crate::plan::manifest::PlanManifest;

pub fn plan_search_dirs(config: &GlobalConfig) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    dirs.push(config.general.plans_dir.clone());
    for extra_dir in &config.general.extra_plans_dirs {
        dirs.push(extra_dir.clone());
    }

    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }

    dirs
}

pub fn setup_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    let mut store = LocalPartStore::new();
    store.add_search_dir(config.general.parts_dir.clone());
    Ok(store)
}

pub fn resolve_targets(
    targets: &[String],
    index: &PlanIndex,
    plan_dirs: &[PathBuf],
) -> Result<HashSet<PathBuf>> {
    let mut plans_to_build = HashSet::new();

    for target in targets {
        let clean_target = target.trim();
        if clean_target.is_empty() {
            continue;
        }

        if let Some(path) = index.path_for(clean_target) {
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
                for plans_dir in plan_dirs {
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
