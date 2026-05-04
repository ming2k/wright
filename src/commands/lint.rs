use crate::builder::orchestrator::{self, setup_resolver};
use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::part::version;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

pub async fn execute_lint(
    targets: Vec<String>,
    _recursive: bool,
    config: &GlobalConfig,
) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let all_plan_paths = collect_plan_files(&resolver.plans_dirs)?;
    let mut local_index = build_local_plan_index(&all_plan_paths);

    let mut plan_targets = Vec::new();

    if targets.is_empty() {
        plan_targets.extend(all_plan_paths);
    } else {
        for target in targets {
            if let Some(path) = all_plans.get(&target) {
                plan_targets.push(path.clone());
            } else {
                let path = PathBuf::from(&target);
                if path.is_file() {
                    plan_targets.push(path);
                } else if path.join("plan.toml").is_file() {
                    plan_targets.push(path.join("plan.toml"));
                } else {
                    let mut found = false;
                    for plans_dir in &resolver.plans_dirs {
                        let candidate = plans_dir.join(&target).join("plan.toml");
                        if candidate.is_file() {
                            plan_targets.push(candidate);
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        warn!("Target not found: {}", target);
                    }
                }
            }
        }
    }

    let mut failed = 0;
    let mut parsed_targets = Vec::new();

    // 1. Lint individual plan manifests
    for path in &plan_targets {
        match PlanManifest::from_file(path) {
            Ok(m) => {
                info!("Plan [OK]: {} ({})", m.plan.name, path.display());
                local_index.insert(
                    m.plan.name.clone(),
                    LocalPlanOutputs {
                        path: path.clone(),
                        outputs: output_names(&m),
                    },
                );
                parsed_targets.push((path.clone(), m));
            }
            Err(e) => {
                report_lint_error(format!("Plan [ERR]: {} - {}", path.display(), e));
                failed += 1;
            }
        }
    }

    // 2. Lint dependency references against the local plan/output index.
    for (path, manifest) in &parsed_targets {
        for message in validate_local_dependency_refs(path, manifest, &local_index) {
            report_lint_error(message);
            failed += 1;
        }
    }

    // 3. Lint dependency graph when targets are specified
    let plan_names: Vec<String> = parsed_targets
        .iter()
        .map(|(_, m)| m.plan.name.clone())
        .collect();

    if !plan_names.is_empty() {
        if let Err(e) = orchestrator::lint_dependency_graph_for_targets(config, &plan_names) {
            report_lint_error(format!("Dependency graph analysis failed: {}", e));
            failed += 1;
        }
    }

    if failed > 0 {
        return Err(crate::error::WrightError::ValidationError(format!(
            "Lint failed with {} issue(s)",
            failed
        )));
    }
    info!("Lint passed: {} plans.", plan_targets.len());
    Ok(())
}

fn report_lint_error(message: impl AsRef<str>) {
    let message = message.as_ref();
    error!("{}", message);
    eprintln!("error: {}", message);
}

#[derive(Debug, Clone)]
struct LocalPlanOutputs {
    path: PathBuf,
    outputs: HashSet<String>,
}

fn collect_plan_files(plans_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for root in plans_dirs {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root) {
            let entry = entry.map_err(|e| WrightError::IoError(std::io::Error::other(e)))?;
            if entry.file_name() == "plan.toml" {
                let path = entry.path().to_path_buf();
                if seen.insert(path.clone()) {
                    paths.push(path);
                }
            }
        }
    }

    paths.sort();
    Ok(paths)
}

fn build_local_plan_index(paths: &[PathBuf]) -> HashMap<String, LocalPlanOutputs> {
    let mut index = HashMap::new();
    for path in paths {
        if let Ok(manifest) = PlanManifest::from_file(path) {
            index.insert(
                manifest.plan.name.clone(),
                LocalPlanOutputs {
                    path: path.clone(),
                    outputs: output_names(&manifest),
                },
            );
        }
    }
    index
}

fn output_names(manifest: &PlanManifest) -> HashSet<String> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref outputs)) => {
            outputs.iter().map(|(name, _)| name.clone()).collect()
        }
        _ => HashSet::from([manifest.plan.name.clone()]),
    }
}

fn dependency_entries(manifest: &PlanManifest) -> Vec<(String, String)> {
    let mut entries = Vec::new();

    for dep in &manifest.build_deps {
        entries.push(("build_deps".to_string(), dep.clone()));
    }
    for dep in &manifest.link_deps {
        entries.push(("link_deps".to_string(), dep.clone()));
    }

    match manifest.outputs {
        Some(OutputConfig::Multi(ref outputs)) => {
            for (output_name, output) in outputs {
                for dep in &output.runtime_deps {
                    entries.push((format!("output.{}.runtime_deps", output_name), dep.clone()));
                }
            }
        }
        _ => {
            for dep in &manifest.runtime_deps {
                entries.push(("output.runtime_deps".to_string(), dep.clone()));
            }
        }
    }

    entries
}

fn validate_local_dependency_refs(
    path: &Path,
    manifest: &PlanManifest,
    local_index: &HashMap<String, LocalPlanOutputs>,
) -> Vec<String> {
    let mut messages = Vec::new();

    for (location, dep) in dependency_entries(manifest) {
        let (dep_plan, dep_output, _) = match version::parse_dependency_ref(&dep) {
            Ok(parsed) => parsed,
            Err(e) => {
                messages.push(format!(
                    "{}: {} dependency '{}': {}",
                    path.display(),
                    location,
                    dep,
                    e
                ));
                continue;
            }
        };

        let Some(target) = local_index.get(&dep_plan) else {
            messages.push(format!(
                "{}: {} dependency '{}' references missing local plan '{}'",
                path.display(),
                location,
                dep,
                dep_plan
            ));
            continue;
        };

        if !target.outputs.contains(&dep_output) {
            let mut outputs: Vec<&str> = target.outputs.iter().map(String::as_str).collect();
            outputs.sort_unstable();
            messages.push(format!(
                "{}: {} dependency '{}' references missing output '{}:{}' (defined outputs in {}: {})",
                path.display(),
                location,
                dep,
                dep_plan,
                dep_output,
                target.path.display(),
                outputs.join(", ")
            ));
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(toml: &str) -> PlanManifest {
        PlanManifest::parse(toml).unwrap()
    }

    #[test]
    fn validates_dependency_output_exists() {
        let consumer = manifest(
            r#"
name = "consumer"
version = "1.0"
release = 1
description = "consumer"
license = "MIT"
arch = "x86_64"
link_deps = ["provider:libs"]
"#,
        );
        let provider = manifest(
            r#"
name = "provider"
version = "1.0"
release = 1
description = "provider"
license = "MIT"
arch = "x86_64"

[[output]]
name = "libs"
description = "provider libraries"
include = ["/usr/lib/.*"]
"#,
        );
        let index = HashMap::from([(
            "provider".to_string(),
            LocalPlanOutputs {
                path: PathBuf::from("/plans/provider/plan.toml"),
                outputs: output_names(&provider),
            },
        )]);

        assert!(validate_local_dependency_refs(
            Path::new("/plans/consumer/plan.toml"),
            &consumer,
            &index
        )
        .is_empty());
    }

    #[test]
    fn rejects_dependency_missing_output() {
        let consumer = manifest(
            r#"
name = "consumer"
version = "1.0"
release = 1
description = "consumer"
license = "MIT"
arch = "x86_64"
link_deps = ["provider:default"]
"#,
        );
        let provider = manifest(
            r#"
name = "provider"
version = "1.0"
release = 1
description = "provider"
license = "MIT"
arch = "x86_64"

[[output]]
name = "libs"
description = "provider libraries"
include = ["/usr/lib/.*"]
"#,
        );
        let index = HashMap::from([(
            "provider".to_string(),
            LocalPlanOutputs {
                path: PathBuf::from("/plans/provider/plan.toml"),
                outputs: output_names(&provider),
            },
        )]);

        let messages = validate_local_dependency_refs(
            Path::new("/plans/consumer/plan.toml"),
            &consumer,
            &index,
        );
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("missing output 'provider:provider'"));
    }
}
