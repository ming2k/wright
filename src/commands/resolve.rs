use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::PathBuf;
use anyhow::Result;
use owo_colors::OwoColorize;

use crate::builder::orchestrator::{self, DependentsMode, MatchPolicy, ResolveOptions};
use crate::cli::resolve::{DomainArg, MatchPolicyArg, ResolveArgs};
use crate::config::GlobalConfig;
use crate::part::version;
use crate::plan::manifest::PlanManifest;

pub async fn execute_resolve(args: ResolveArgs, config: &GlobalConfig) -> Result<()> {
    let is_tty = std::io::stdout().is_terminal();

    if is_tty && !args.exclude_targets && (args.tree || (args.deps.is_none() && args.rdeps.is_none())) {
        render_interactive_trees(&args, config).await?;
        return Ok(());
    }

    if is_tty && !args.exclude_targets && (args.deps.is_some() || args.rdeps.is_some()) {
        render_interactive_trees(&args, config).await?;
        return Ok(());
    }

    render_list_output(args, config).await
}

async fn render_interactive_trees(args: &ResolveArgs, config: &GlobalConfig) -> Result<()> {
    let resolver = orchestrator::setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;

    let mut rdeps_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (name, path) in &all_plans {
        if let Ok(m) = PlanManifest::from_file(path) {
            for (dep_raw, kind) in m.all_dependencies() {
                let dep_name = version::parse_dependency(&dep_raw)
                    .map(|(n, _)| n)
                    .unwrap_or(dep_raw);
                rdeps_map
                    .entry(dep_name)
                    .or_default()
                    .push((name.clone(), kind));
            }
        }
    }

    let effective_depth = match args.depth {
        Some(0) => usize::MAX,
        Some(d) => d,
        None => 1,
    };

    for (idx, target) in args.targets.iter().enumerate() {
        if idx > 0 {
            println!();
        }

        let show_dependencies = (args.deps.is_none() && args.rdeps.is_none()) || args.deps.is_some();
        if show_dependencies {
            println!("{}", "Dependencies:".bold().cyan());
            print!("{}", target.bold().green());
            let mut visited = HashSet::new();
            visited.insert(target.to_string());
            if !print_dependency_tree(
                target,
                &all_plans,
                "",
                1,
                effective_depth,
                &mut visited,
                args.deps,
            )? {
                println!(" {}", "(none)".dimmed());
            } else {
                println!();
            }
        }

        let show_dependents = (args.deps.is_none() && args.rdeps.is_none()) || args.rdeps.is_some();
        if show_dependents {
            println!("{}", "Dependents:".bold().cyan());
            print!("{}", target.bold().green());
            let mut visited = HashSet::new();
            visited.insert(target.to_string());
            if !print_dependent_tree(
                target,
                &rdeps_map,
                "",
                1,
                effective_depth,
                &mut visited,
                args.rdeps,
            )? {
                println!(" {}", "(none)".dimmed());
            } else {
                println!();
            }
        }
    }
    Ok(())
}

async fn render_list_output(args: ResolveArgs, config: &GlobalConfig) -> Result<()> {
    let deps = args.deps.map(|d| match d {
        DomainArg::Link => DependentsMode::Link,
        DomainArg::Runtime => DependentsMode::Runtime,
        DomainArg::Build => DependentsMode::Build,
        DomainArg::All => DependentsMode::All,
    });

    let rdeps = args.rdeps.map(|d| match d {
        DomainArg::Link => DependentsMode::Link,
        DomainArg::Runtime => DependentsMode::Runtime,
        DomainArg::Build => DependentsMode::Build,
        DomainArg::All => DependentsMode::All,
    });

    let match_policies = args
        .match_policies
        .iter()
        .map(|p| match p {
            MatchPolicyArg::All => MatchPolicy::All,
            MatchPolicyArg::Missing => MatchPolicy::Missing,
            MatchPolicyArg::Outdated => MatchPolicy::Outdated,
            MatchPolicyArg::Installed => MatchPolicy::Installed,
        })
        .collect();

    let effective_depth = match args.depth {
        Some(value) => Some(value),
        None if rdeps.is_some() => Some(1),
        None => Some(0),
    };

    let names = orchestrator::resolve_build_set(
        config,
        args.targets,
        ResolveOptions {
            deps,
            rdeps,
            match_policies,
            depth: effective_depth,
            include_targets: !args.exclude_targets,
            preserve_targets: false,
        },
    ).await?;

    for name in &names {
        println!("{}", name);
    }

    Ok(())
}

fn print_dependency_tree(
    name: &str,
    all_plans: &HashMap<String, PathBuf>,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    visited: &mut HashSet<String>,
    filter: Option<DomainArg>,
) -> Result<bool> {
    if current_depth > max_depth {
        return Ok(false);
    }
    let path = match all_plans.get(name) {
        Some(p) => p,
        None => return Ok(false),
    };
    let manifest = PlanManifest::from_file(path)?;
    let mut deps = manifest.all_dependencies();
    if let Some(domain) = filter {
        deps.retain(|(_, kind)| match domain {
            DomainArg::Link => kind == "link",
            DomainArg::Runtime => kind == "runtime",
            DomainArg::Build => kind == "build",
            DomainArg::All => true,
        });
    }
    if deps.is_empty() {
        return Ok(false);
    }
    for (i, (dep_raw, kind)) in deps.iter().enumerate() {
        let dep_name = version::parse_dependency(dep_raw)
            .map(|(n, _)| n)
            .unwrap_or_else(|_| dep_raw.clone());
        let last_child = i == deps.len() - 1;
        let connector = if last_child { "└── " } else { "├── " };
        print!("\n{}{}{}: {}", prefix, connector, kind.dimmed(), dep_name);
        if visited.contains(&dep_name) {
            print!(" {}", "(*)".dimmed());
            continue;
        }
        visited.insert(dep_name.clone());
        let new_prefix = format!("{}{}", prefix, if last_child { "    " } else { "│   " });
        print_dependency_tree(&dep_name, all_plans, &new_prefix, current_depth + 1, max_depth, visited, filter)?;
    }
    Ok(true)
}

fn print_dependent_tree(
    name: &str,
    rdeps_map: &HashMap<String, Vec<(String, String)>>,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
    visited: &mut HashSet<String>,
    filter: Option<DomainArg>,
) -> Result<bool> {
    if current_depth > max_depth {
        return Ok(false);
    }
    let rdeps_full = match rdeps_map.get(name) {
        Some(r) => r,
        None => return Ok(false),
    };
    let mut rdeps = rdeps_full.clone();
    if let Some(domain) = filter {
        rdeps.retain(|(_, kind)| match domain {
            DomainArg::Link => kind == "link",
            DomainArg::Runtime => kind == "runtime",
            DomainArg::Build => kind == "build",
            DomainArg::All => true,
        });
    }
    if rdeps.is_empty() {
        return Ok(false);
    }
    for (i, (child_name, kind)) in rdeps.iter().enumerate() {
        let last_child = i == rdeps.len() - 1;
        let connector = if last_child { "└── " } else { "├── " };
        print!("\n{}{}{}: {}", prefix, connector, kind.dimmed(), child_name);
        if visited.contains(child_name) {
            print!(" {}", "(*)".dimmed());
            continue;
        }
        visited.insert(child_name.clone());
        let new_prefix = format!("{}{}", prefix, if last_child { "    " } else { "│   " });
        print_dependent_tree(child_name, rdeps_map, &new_prefix, current_depth + 1, max_depth, visited, filter)?;
    }
    Ok(true)
}
