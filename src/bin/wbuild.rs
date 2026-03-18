use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use wright::builder::orchestrator::{self, BuildOptions, DependencyMode};
use wright::cli::wbuild::{Cli, Commands, DepsMode};
use wright::config::GlobalConfig;
use wright::part::version;
use wright::plan::manifest::PlanManifest;

#[derive(Default)]
struct PlanTreeStats {
    total: usize,
    max_depth_seen: usize,
    cycles: usize,
    repeated: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose > 1 {
        EnvFilter::new("trace")
    } else if cli.verbose > 0 {
        EnvFilter::new("debug")
    } else if cli.quiet {
        EnvFilter::new("warn")
    } else {
        EnvFilter::new("info")
    };

    if cli.verbose > 0 {
        // Verbose: show full format with timestamps and module paths for debugging
        tracing_subscriber::fmt()
            .with_writer(wright::util::progress::MultiProgressWriter)
            .with_env_filter(filter)
            .init();
    } else {
        // Default/quiet: clean format without timestamps or module paths
        tracing_subscriber::fmt()
            .with_writer(wright::util::progress::MultiProgressWriter)
            .without_time()
            .with_target(false)
            .with_level(true)
            .with_env_filter(filter)
            .init();
    }
    let config = GlobalConfig::load(cli.config.as_deref()).context("failed to load config")?;

    match cli.command {
        Commands::Run {
            targets,
            stage,
            skip_check,
            clean,
            force,
            dockyards,
            rebuild_dependents,
            install,
            depth,
            include_self,
            deps,
            include_dependents,
            mvp,
        } => {
            let deps_mode = match deps.unwrap_or(DepsMode::None) {
                DepsMode::None => DependencyMode::None,
                DepsMode::Missing => DependencyMode::Missing,
                DepsMode::Sync => DependencyMode::Sync,
                DepsMode::All => DependencyMode::All,
            };
            // CLI --dockyards 0 means "use config default"; config dockyards 0 means auto-detect.
            let effective_dockyards = if dockyards != 0 {
                dockyards
            } else {
                config.build.dockyards
            };
            Ok(orchestrator::run_build(
                &config,
                targets,
                BuildOptions {
                    stages: stage,
                    fetch_only: false,
                    clean,
                    force,
                    dockyards: effective_dockyards,
                    rebuild_dependents,
                    deps_mode,
                    install,
                    depth: Some(depth),
                    checksum: false,
                    lint: false,
                    skip_check,
                    verbose: cli.verbose > 0,
                    quiet: cli.quiet,
                    include_self,
                    include_dependents,
                    mvp,
                    // Config nproc_per_dockyard is a static override; None means the
                    // scheduler computes it dynamically as total_cpus / active_dockyards.
                    nproc_per_dockyard: config.build.nproc_per_dockyard,
                },
            )?)
        }
        Commands::Check { targets } => Ok(orchestrator::run_build(
            &config,
            targets,
            BuildOptions {
                lint: true,
                ..Default::default()
            },
        )?),
        Commands::Fetch { targets } => Ok(orchestrator::run_build(
            &config,
            targets,
            BuildOptions {
                fetch_only: true,
                ..Default::default()
            },
        )?),
        Commands::Checksum { targets } => Ok(orchestrator::run_build(
            &config,
            targets,
            BuildOptions {
                checksum: true,
                ..Default::default()
            },
        )?),
        Commands::Deps { target, depth } => {
            // Static analysis of plans in hold tree
            let resolver = wright::builder::orchestrator::setup_resolver(&config)?;
            let all_plans = resolver.get_all_plans()?;

            println!(
                "Plan dependency tree for: {} (source: hold-tree plan.toml)",
                target
            );
            let stats = print_plan_tree(
                &target,
                &all_plans,
                "",
                1,
                if depth == 0 { usize::MAX } else { depth },
            )?;
            println!(
                "\n{} parts, max depth {}, {} repeated, {} cycles",
                stats.total, stats.max_depth_seen, stats.repeated, stats.cycles
            );
            println!("\nSource: hold-tree plan.toml");
            Ok(())
        }
    }
}

fn print_plan_tree(
    name: &str,
    all_plans: &HashMap<String, PathBuf>,
    prefix: &str,
    current_depth: usize,
    max_depth: usize,
) -> Result<PlanTreeStats> {
    let mut visited = std::collections::HashSet::new();
    let mut ancestors = std::collections::HashSet::new();
    let mut stats = PlanTreeStats {
        total: 1,
        max_depth_seen: 0,
        cycles: 0,
        repeated: 0,
    };
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
    visited: &mut std::collections::HashSet<String>,
    ancestors: &mut std::collections::HashSet<String>,
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
            // True cycle: check if the dep has [mvp.dependencies] that could break it
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
