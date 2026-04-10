use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use wright::builder::orchestrator::{
    self, BuildOptions, DependencyMode, DependentsMode, ResolveOptions,
};
use wright::cli::wbuild::{Cli, Commands, DependentsMode as DependentsModeArg, DepsMode};
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
            resume,
            dockyards,
            mvp,
            print_archives,
            clear_sessions,
        } => {
            let _command_lock =
                wright::util::lock::acquire_named_lock(&config.general.db_path, "wbuild")
                    .context("failed to acquire wbuild command lock")?;

            if clear_sessions {
                let db = wright::database::Database::open(&config.general.db_path)
                    .context("failed to open database")?;
                let count = db.clear_all_sessions()?;
                info!("Cleared {} build session(s)", count);
                return Ok(());
            }

            // Collect targets from command line args + stdin (when piped).
            let mut all_targets = targets;
            if !io::stdin().is_terminal() {
                for line in io::stdin().lock().lines() {
                    let line = line.context("failed to read target from stdin")?;
                    let trimmed = line.trim().to_string();
                    if !trimmed.is_empty() {
                        all_targets.push(trimmed);
                    }
                }
            }

            // CLI --dockyards 0 means "use config default"; config dockyards 0 means auto-detect.
            let effective_dockyards = if dockyards != 0 {
                dockyards
            } else {
                config.build.dockyards
            };
            Ok(orchestrator::run_build(
                &config,
                all_targets,
                BuildOptions {
                    stages: stage,
                    fetch_only: false,
                    clean,
                    force,
                    resume: resume.map(|h| if h.is_empty() { None } else { Some(h) }),
                    dockyards: effective_dockyards,
                    checksum: false,
                    lint: false,
                    skip_check,
                    verbose: cli.verbose > 0,
                    quiet: cli.quiet,
                    mvp,
                    print_archives,
                    // Config nproc_per_dockyard is a static override; None means the
                    // scheduler computes it dynamically as total_cpus / active_dockyards.
                    nproc_per_dockyard: config.build.nproc_per_dockyard,
                },
            )?)
        }
        Commands::Resolve {
            targets,
            include_self,
            deps,
            dependents,
            depth,
            tree,
        } => {
            if tree {
                let target = targets
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--tree requires at least one target"))?;
                let effective_depth = match depth {
                    Some(0) | None => usize::MAX,
                    Some(d) => d,
                };
                let resolver = wright::builder::orchestrator::setup_resolver(&config)?;
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
                Ok(())
            } else {
                let deps_mode = match deps.unwrap_or(DepsMode::None) {
                    DepsMode::None => DependencyMode::None,
                    DepsMode::Missing => DependencyMode::Missing,
                    DepsMode::Sync => DependencyMode::Sync,
                    DepsMode::All => DependencyMode::All,
                };
                let dependents_mode = match dependents {
                    None => DependentsMode::None,
                    Some(DependentsModeArg::Link) => DependentsMode::Link,
                    Some(DependentsModeArg::All) => DependentsMode::All,
                };
                let effective_depth = match depth {
                    Some(value) => Some(value),
                    None if dependents_mode != DependentsMode::None => Some(1),
                    None => Some(0),
                };

                let names = orchestrator::resolve_build_set(
                    &config,
                    targets,
                    ResolveOptions {
                        deps_mode,
                        dependents_mode,
                        depth: effective_depth,
                        include_self,
                        install: false,
                    },
                )?;

                for name in &names {
                    println!("{}", name);
                }
                Ok(())
            }
        }
        Commands::Check { targets } => {
            let _command_lock =
                wright::util::lock::acquire_named_lock(&config.general.db_path, "wbuild")
                    .context("failed to acquire wbuild command lock")?;

            Ok(orchestrator::run_build(
                &config,
                targets,
                BuildOptions {
                    lint: true,
                    ..Default::default()
                },
            )?)
        }
        Commands::Fetch { targets } => {
            let _command_lock =
                wright::util::lock::acquire_named_lock(&config.general.db_path, "wbuild")
                    .context("failed to acquire wbuild command lock")?;

            Ok(orchestrator::run_build(
                &config,
                targets,
                BuildOptions {
                    fetch_only: true,
                    ..Default::default()
                },
            )?)
        }
        Commands::Checksum { targets } => {
            let _command_lock =
                wright::util::lock::acquire_named_lock(&config.general.db_path, "wbuild")
                    .context("failed to acquire wbuild command lock")?;

            Ok(orchestrator::run_build(
                &config,
                targets,
                BuildOptions {
                    checksum: true,
                    ..Default::default()
                },
            )?)
        }
        Commands::Prune {
            untracked,
            latest,
            apply,
        } => {
            prune_archives(&config, untracked, latest, apply)?;
            Ok(())
        }
    }
}

fn prune_archives(config: &GlobalConfig, prune_untracked: bool, keep_latest: bool, apply: bool) -> Result<()> {
    if !prune_untracked && !keep_latest {
        anyhow::bail!("nothing to do: pass --untracked and/or --latest");
    }

    let inventory = wright::inventory::db::InventoryDb::open(&config.general.inventory_db_path)
        .context("failed to open local inventory database")?;
    let archives_dir = &config.general.components_dir;
    std::fs::create_dir_all(archives_dir)
        .with_context(|| format!("failed to create {}", archives_dir.display()))?;

    let installed_db = wright::database::Database::open(&config.general.db_path)
        .context("failed to open installed-part database")?;

    let report = if apply {
        wright::inventory::prune::apply_prune(
            &inventory,
            &installed_db,
            archives_dir,
            prune_untracked,
            keep_latest,
        )
        .context("prune failed")?
    } else {
        // Dry-run: reconcile stale DB rows but don't delete files.
        let stale_db_rows = inventory
            .remove_missing_files(archives_dir)
            .context("failed to reconcile missing archive files")?;
        let mut report = wright::inventory::prune::plan_prune(
            &inventory,
            &installed_db,
            archives_dir,
            prune_untracked,
            keep_latest,
        )
        .context("prune planning failed")?;
        report.stale_db_rows = stale_db_rows;
        report
    };

    for filename in &report.stale_db_rows {
        println!("inventory-stale: {}", filename);
    }
    for path in &report.untracked {
        println!("prune untracked: {}", path.display());
    }
    for stale in &report.stale_tracked {
        println!(
            "prune tracked: {} ({} {}-{})",
            stale.path.display(),
            stale.name,
            stale.version,
            stale.release
        );
    }

    if report.untracked.is_empty() && report.stale_tracked.is_empty() {
        println!("nothing to prune");
        return Ok(());
    }

    if !apply {
        println!("dry-run only; rerun with --apply to delete the listed archives");
    }

    Ok(())
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
            // True cycle: check if the dep has mvp.toml that could break it
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
