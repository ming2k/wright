use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;

use crate::builder::orchestrator::{self, DependencyMode, DependentsMode, ResolveOptions};
use crate::cli::resolve::{DependentsModeArg, DepsMode, ResolveArgs};
use crate::config::GlobalConfig;
use crate::part::version;
use crate::plan::manifest::PlanManifest;

pub fn execute_resolve(args: ResolveArgs, config: &GlobalConfig) -> Result<()> {
    if args.tree {
        let target = args
            .targets
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("--tree requires at least one target"))?;
        let effective_depth = match args.depth {
            Some(0) | None => usize::MAX,
            Some(d) => d,
        };
        let resolver = orchestrator::setup_resolver(config)?;
        let all_plans = resolver.get_all_plans()?;
        println!(
            "Plan dependency tree for: {} (source: hold-tree plan.toml)",
            target
        );
        let stats = print_plan_tree(&target, &all_plans, "", 1, effective_depth)?;
        println!(
            "\n{} parts, max depth {}, {} repeated, {} cycles",
            stats.total, stats.max_depth_seen, stats.repeated, stats.cycles
        );
        println!("\nSource: hold-tree plan.toml");
    } else {
        let deps_mode = match args.deps.unwrap_or(DepsMode::None) {
            DepsMode::None => DependencyMode::None,
            DepsMode::Missing => DependencyMode::Missing,
            DepsMode::Sync => DependencyMode::Sync,
            DepsMode::All => DependencyMode::All,
        };
        let dependents_mode = match args.dependents {
            None => DependentsMode::None,
            Some(DependentsModeArg::Link) => DependentsMode::Link,
            Some(DependentsModeArg::All) => DependentsMode::All,
        };
        let effective_depth = match args.depth {
            Some(value) => Some(value),
            None if dependents_mode != DependentsMode::None => Some(1),
            None => Some(0),
        };

        let names = orchestrator::resolve_build_set(
            config,
            args.targets,
            ResolveOptions {
                deps_mode,
                dependents_mode,
                depth: effective_depth,
                include_targets: args.include_targets,
            },
        )?;

        for name in &names {
            println!("{}", name);
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct PlanTreeStats {
    pub total: usize,
    pub max_depth_seen: usize,
    pub cycles: usize,
    pub repeated: usize,
}

pub fn print_plan_tree(
    name: &str,
    all_plans: &HashMap<String, PathBuf>,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
) -> Result<PlanTreeStats> {
    let mut visited = HashSet::new();
    let mut ancestors = HashSet::new();
    let mut stats = PlanTreeStats::default();
    visited.insert(name.to_string());
    ancestors.insert(name.to_string());
    print_plan_tree_inner(
        name,
        all_plans,
        prefix,
        current_depth,
        max_depth,
        &mut visited,
        &mut ancestors,
        &mut stats,
    )?;
    Ok(stats)
}

fn print_plan_tree_inner(
    name: &str,
    all_plans: &HashMap<String, PathBuf>,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    visited: &mut HashSet<String>,
    ancestors: &mut HashSet<String>,
    stats: &mut PlanTreeStats,
) -> Result<()> {
    if current_depth > max_depth {
        return Ok(());
    }

    let path = all_plans
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Plan '{}' not found in hold tree", name))?;

    let manifest = PlanManifest::from_file(path)?;

    let mut all_deps = Vec::new();
    all_deps.extend(manifest.dependencies.build.iter().cloned());
    all_deps.extend(manifest.dependencies.link.iter().cloned());
    all_deps.extend(manifest.dependencies.runtime.iter().cloned());

    for (i, dep) in all_deps.iter().enumerate() {
        let dep_name = version::parse_dependency(dep)
            .unwrap_or_else(|_| (dep.clone(), None))
            .0;
        let is_last = i == all_deps.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        stats.total += 1;
        if current_depth > stats.max_depth_seen {
            stats.max_depth_seen = current_depth;
        }

        if ancestors.contains(&dep_name) {
            stats.cycles += 1;
            let cycle_note = all_plans
                .get(&dep_name)
                .and_then(|p| PlanManifest::from_file(p).ok())
                .and_then(|m| m.mvp)
                .and_then(|mvp| mvp.dependencies)
                .map(|_| " (cycle → resolvable via mvp)")
                .unwrap_or(" (cycle → no mvp defined!)");
            println!("{}{}{}{}", prefix, connector, dep, cycle_note);
        } else if visited.contains(&dep_name) {
            stats.repeated += 1;
            println!("{}{}{} (*)", prefix, connector, dep);
        } else {
            println!("{}{}{}", prefix, connector, dep);
            if all_plans.contains_key(&dep_name) {
                visited.insert(dep_name.clone());
                ancestors.insert(dep_name.clone());
                let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
                print_plan_tree_inner(
                    &dep_name,
                    all_plans,
                    &new_prefix,
                    current_depth + 1,
                    max_depth,
                    visited,
                    ancestors,
                    stats,
                )?;
                ancestors.remove(&dep_name);
            }
        }
    }

    Ok(())
}
