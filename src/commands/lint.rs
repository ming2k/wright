use crate::builder::orchestrator;
use crate::config::GlobalConfig;
use crate::error::Result;
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
    let plan_dirs = orchestrator::plan_search_dirs(config);
    let all_plans = crate::plan::discovery::get_all_plans(&plan_dirs)?;
    let all_plan_paths = crate::plan::discovery::collect_plan_files(&plan_dirs)?;
    let mut local_index = build_local_plan_index(&all_plan_paths);

    let mut plan_targets = Vec::new();

    if targets.is_empty() {
        plan_targets.extend(all_plan_paths.iter().cloned());
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
                    for plans_dir in &plan_dirs {
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

    for message in validate_global_names(&all_plan_paths) {
        report_lint_error(message);
        failed += 1;
    }

    // 1. Lint individual plan manifests
    for path in &plan_targets {
        match PlanManifest::from_file(path) {
            Ok(m) => {
                info!("Plan [OK]: {} ({})", m.metadata.name, path.display());
                local_index.insert(
                    m.metadata.name.clone(),
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
        .map(|(_, m)| m.metadata.name.clone())
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

fn build_local_plan_index(paths: &[PathBuf]) -> HashMap<String, LocalPlanOutputs> {
    let mut index = HashMap::new();
    for path in paths {
        if let Ok(manifest) = PlanManifest::from_file(path) {
            index.insert(
                manifest.metadata.name.clone(),
                LocalPlanOutputs {
                    path: path.clone(),
                    outputs: output_names(&manifest),
                },
            );
        }
    }
    index
}

fn validate_global_names(paths: &[PathBuf]) -> Vec<String> {
    let mut messages = Vec::new();
    let mut plan_owners: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut output_owners: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();

    for path in paths {
        let Ok(manifest) = PlanManifest::from_file(path) else {
            continue;
        };
        let plan_name = manifest.metadata.name.clone();
        plan_owners
            .entry(plan_name.clone())
            .or_default()
            .push(path.clone());
        for output_name in output_names(&manifest) {
            output_owners
                .entry(output_name)
                .or_default()
                .push((plan_name.clone(), path.clone()));
        }
    }

    for (name, owners) in &plan_owners {
        if owners.len() > 1 {
            let paths = owners
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            messages.push(format!("duplicate plan name '{}': {}", name, paths));
        }
    }

    for (name, owners) in &output_owners {
        let mut unique_plan_names = owners
            .iter()
            .map(|(plan_name, _)| plan_name.as_str())
            .collect::<Vec<_>>();
        unique_plan_names.sort_unstable();
        unique_plan_names.dedup();
        if unique_plan_names.len() > 1 {
            let locations = owners
                .iter()
                .map(|(plan_name, path)| format!("{} in {}", plan_name, path.display()))
                .collect::<Vec<_>>()
                .join(", ");
            messages.push(format!("duplicate output name '{}': {}", name, locations));
        }
    }

    for (plan_name, plan_paths) in &plan_owners {
        let Some(outputs) = output_owners.get(plan_name) else {
            continue;
        };
        let conflicts = outputs
            .iter()
            .filter(|(owner_plan, _)| owner_plan != plan_name)
            .map(|(owner_plan, path)| format!("output of '{}' in {}", owner_plan, path.display()))
            .collect::<Vec<_>>();
        if !conflicts.is_empty() {
            let plan_locations = plan_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            messages.push(format!(
                "plan name '{}' conflicts with {} (plan defined in {})",
                plan_name,
                conflicts.join(", "),
                plan_locations
            ));
        }
    }

    messages.sort();
    messages
}

fn output_names(manifest: &PlanManifest) -> HashSet<String> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref outputs)) => {
            outputs.iter().map(|(name, _)| name.clone()).collect()
        }
        _ => HashSet::from([manifest.metadata.name.clone()]),
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
        let (dep_ref, _) = match version::parse_dependency_ref(&dep) {
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

        let dep_plan = dep_ref.plan().to_string();

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

        if let Some(dep_output) = dep_ref.output() {
            if !target.outputs.contains(dep_output) {
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
link_deps = ["provider:provider"]
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

    #[test]
    fn rejects_global_plan_and_output_name_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = tmp.path().join("alpha/plan.toml");
        let beta = tmp.path().join("beta/plan.toml");
        std::fs::create_dir_all(alpha.parent().unwrap()).unwrap();
        std::fs::create_dir_all(beta.parent().unwrap()).unwrap();
        std::fs::write(
            &alpha,
            r#"
name = "alpha"
version = "1.0"
release = 1
description = "alpha"
license = "MIT"
arch = "x86_64"

[[output]]
name = "beta"
description = "conflicts with beta plan name"
include = ["/usr/bin/alpha"]
"#,
        )
        .unwrap();
        std::fs::write(
            &beta,
            r#"
name = "beta"
version = "1.0"
release = 1
description = "beta"
license = "MIT"
arch = "x86_64"
"#,
        )
        .unwrap();

        let messages = validate_global_names(&[alpha, beta]);
        assert!(messages
            .iter()
            .any(|message| message.contains("plan name 'beta' conflicts")));
    }

    #[test]
    fn rejects_duplicate_output_names_across_plans() {
        let tmp = tempfile::tempdir().unwrap();
        let one = tmp.path().join("one/plan.toml");
        let two = tmp.path().join("two/plan.toml");
        std::fs::create_dir_all(one.parent().unwrap()).unwrap();
        std::fs::create_dir_all(two.parent().unwrap()).unwrap();
        for (path, plan_name) in [(&one, "one"), (&two, "two")] {
            std::fs::write(
                path,
                format!(
                    r#"
name = "{plan_name}"
version = "1.0"
release = 1
description = "{plan_name}"
license = "MIT"
arch = "x86_64"

[[output]]
name = "shared"
description = "shared output name"
include = ["/usr/bin/{plan_name}"]
"#
                ),
            )
            .unwrap();
        }

        let messages = validate_global_names(&[one, two]);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("duplicate output name 'shared'"));
    }
}
