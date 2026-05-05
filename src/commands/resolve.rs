use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::builder::orchestrator::{self, DependentsMode, MatchPolicy, ResolveOptions};
use crate::cli::resolve::{DomainArg, MatchPolicyArg, ResolveArgs};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::version;
use crate::plan::manifest::PlanManifest;
use crate::query::{self, PrefixMode, TreeOptions};

pub async fn execute_resolve(
    args: ResolveArgs,
    config: &GlobalConfig,
    db_path: &Path,
) -> Result<()> {
    if args.installed {
        render_installed_tree(&args, db_path).await
    } else {
        render_build_view(args, config).await
    }
}

// ─── Installed-side tree rendering ───────────────────────────────────────────

async fn render_installed_tree(args: &ResolveArgs, db_path: &Path) -> Result<()> {
    let db = InstalledDb::open(db_path)
        .await
        .context("failed to open installed database")?;
    let color = std::io::stdout().is_terminal();
    let mut buf = Vec::new();

    let max_depth = match args.depth {
        Some(0) => usize::MAX,
        Some(d) => d,
        None => usize::MAX,
    };

    let opts = TreeOptions {
        max_depth,
        filter: None,
        prefix_mode: PrefixMode::Indent,
        prune: &[],
        color,
    };

    for (idx, target) in args.targets.iter().enumerate() {
        if idx > 0 {
            writeln!(buf)?;
        }

        let part = db.get_part(target).await.context("failed to query part")?;
        if part.is_none() {
            eprintln!("part '{}' is not installed", target);
            std::process::exit(1);
        }

        let show_deps = (args.deps.is_none() && args.rdeps.is_none()) || args.deps.is_some();
        let show_rdeps = (args.deps.is_none() && args.rdeps.is_none()) || args.rdeps.is_some();

        if show_deps {
            writeln!(buf, "{}", "Dependencies:".bold().cyan())?;
            writeln!(buf, "{}", target.bold().green())?;
            let stats = query::write_dep_tree(&db, target, &opts, &mut buf).await?;
            stats.write_summary(&mut buf, color).ok();
        }

        if show_rdeps {
            if show_deps {
                writeln!(buf)?;
            }
            writeln!(buf, "{}", "Dependents:".bold().cyan())?;
            writeln!(buf, "{}", target.bold().green())?;
            let stats = query::write_reverse_dep_tree(&db, target, &opts, &mut buf).await?;
            stats.write_summary(&mut buf, color).ok();
        }
    }

    if color {
        writeln!(
            buf,
            "{}",
            "\nSource: local part database (.PARTINFO-derived metadata)".dimmed()
        )
        .ok();
    } else {
        writeln!(
            buf,
            "\nSource: local part database (.PARTINFO-derived metadata)"
        )
        .ok();
    }

    print!("{}", String::from_utf8_lossy(&buf));
    Ok(())
}

// ─── Plan-side tree rendering ────────────────────────────────────────────────

async fn render_build_view(args: ResolveArgs, config: &GlobalConfig) -> Result<()> {
    let is_tty = std::io::stdout().is_terminal();

    if is_tty && !args.exclude_targets {
        render_plan_tree(args, config).await?;
        return Ok(());
    }

    render_list_output(args, config).await
}

async fn render_plan_tree(args: ResolveArgs, config: &GlobalConfig) -> Result<()> {
    let plan_dirs = orchestrator::plan_search_dirs(config);
    let all_plans = crate::plan::discovery::get_all_plans(&plan_dirs)?;

    let mut rdeps_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (name, path) in &all_plans {
        if let Ok(m) = PlanManifest::from_file(path) {
            for (dep_raw, kind) in m.all_dependencies() {
                let (dep_name, _) = version::parse_dep_ref(&dep_raw);
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

    let color = std::io::stdout().is_terminal();

    for (idx, target) in args.targets.iter().enumerate() {
        if idx > 0 {
            println!();
        }

        let show_dependencies =
            (args.deps.is_none() && args.rdeps.is_none()) || args.deps.is_some();
        if show_dependencies {
            if color {
                println!("{}", "Dependencies:".bold().cyan());
                print!("{}", target.bold().green());
            } else {
                println!("Dependencies:");
                print!("{}", target);
            }
            let mut visited = HashSet::new();
            visited.insert(target.to_string());
            let dep_ctx = DepTreeCtx {
                all_plans: &all_plans,
                max_depth: effective_depth,
                filter: args.deps,
                color,
            };
            if !print_dependency_tree(target, &dep_ctx, "", 1, &mut visited)? {
                println!(
                    " {}",
                    if color {
                        "(none)".dimmed().to_string()
                    } else {
                        "(none)".to_string()
                    }
                );
            } else {
                println!();
            }
        }

        let show_dependents = (args.deps.is_none() && args.rdeps.is_none()) || args.rdeps.is_some();
        if show_dependents {
            if color {
                println!("{}", "Dependents:".bold().cyan());
                print!("{}", target.bold().green());
            } else {
                println!("Dependents:");
                print!("{}", target);
            }
            let mut visited = HashSet::new();
            visited.insert(target.to_string());
            let rdep_ctx = RdepTreeCtx {
                rdeps_map: &rdeps_map,
                max_depth: effective_depth,
                filter: args.rdeps,
                color,
            };
            if !print_dependent_tree(target, &rdep_ctx, "", 1, &mut visited)? {
                println!(
                    " {}",
                    if color {
                        "(none)".dimmed().to_string()
                    } else {
                        "(none)".to_string()
                    }
                );
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
    )
    .await?;

    for name in &names {
        println!("{}", name);
    }

    Ok(())
}

struct DepTreeCtx<'a> {
    all_plans: &'a HashMap<String, PathBuf>,
    max_depth: usize,
    filter: Option<DomainArg>,
    color: bool,
}

fn print_dependency_tree(
    name: &str,
    ctx: &DepTreeCtx,
    prefix: &str,
    current_depth: usize,
    visited: &mut HashSet<String>,
) -> Result<bool> {
    if current_depth > ctx.max_depth {
        return Ok(false);
    }
    let path = match ctx.all_plans.get(name) {
        Some(p) => p,
        None => return Ok(false),
    };
    let manifest = PlanManifest::from_file(path)?;
    let mut deps = manifest.all_dependencies();
    if let Some(domain) = ctx.filter {
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
        let (dep_name, _) = version::parse_dep_ref(dep_raw);
        let last_child = i == deps.len() - 1;
        let connector = if last_child {
            "└── "
        } else {
            "├── "
        };
        if ctx.color {
            print!("\n{}{}{}: {}", prefix, connector, kind.dimmed(), dep_name);
        } else {
            print!("\n{}{}{}: {}", prefix, connector, kind, dep_name);
        }
        if visited.contains(&dep_name) {
            if ctx.color {
                print!(" {}", "(*)".dimmed());
            } else {
                print!(" (*)");
            }
            continue;
        }
        visited.insert(dep_name.clone());
        let new_prefix = format!("{}{}", prefix, if last_child { "    " } else { "│   " });
        print_dependency_tree(&dep_name, ctx, &new_prefix, current_depth + 1, visited)?;
    }
    Ok(true)
}

struct RdepTreeCtx<'a> {
    rdeps_map: &'a HashMap<String, Vec<(String, String)>>,
    max_depth: usize,
    filter: Option<DomainArg>,
    color: bool,
}

fn print_dependent_tree(
    name: &str,
    ctx: &RdepTreeCtx,
    prefix: &str,
    current_depth: usize,
    visited: &mut HashSet<String>,
) -> Result<bool> {
    if current_depth > ctx.max_depth {
        return Ok(false);
    }
    let rdeps_full = match ctx.rdeps_map.get(name) {
        Some(r) => r,
        None => return Ok(false),
    };
    let mut rdeps = rdeps_full.clone();
    if let Some(domain) = ctx.filter {
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
        let connector = if last_child {
            "└── "
        } else {
            "├── "
        };
        if ctx.color {
            print!("\n{}{}{}: {}", prefix, connector, kind.dimmed(), child_name);
        } else {
            print!("\n{}{}{}: {}", prefix, connector, kind, child_name);
        }
        if visited.contains(child_name) {
            if ctx.color {
                print!(" {}", "(*)".dimmed());
            } else {
                print!(" (*)");
            }
            continue;
        }
        visited.insert(child_name.clone());
        let new_prefix = format!("{}{}", prefix, if last_child { "    " } else { "│   " });
        print_dependent_tree(child_name, ctx, &new_prefix, current_depth + 1, visited)?;
    }
    Ok(true)
}
