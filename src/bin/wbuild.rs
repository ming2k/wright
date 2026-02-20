use std::path::PathBuf;
use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wright::config::GlobalConfig;
use wright::builder::orchestrator::{self, BuildOptions};
use wright::package::manifest::PackageManifest;
use wright::package::version;

#[derive(Parser)]
#[command(name = "wbuild", about = "wright package constructor")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Alternate root directory for file operations
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Path to database file
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Reduce log output (show warnings/errors only)
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Build packages from plans (default operation)
    Run {
        /// Paths to plan directories, part names, or @assemblies
        targets: Vec<String>,

        /// Run all stages up to and including this one, then stop (e.g. configure, compile)
        #[arg(long)]
        until: Option<String>,

        /// Run exactly one lifecycle stage; all others are skipped (requires a previous full build)
        #[arg(long)]
        only: Option<String>,

        /// Remove the build directory before starting
        #[arg(long)]
        clean: bool,

        /// Force rebuild: overwrite existing archive and bypass the build cache
        #[arg(long, short)]
        force: bool,

        /// Max number of concurrent build workers. Only packages with no
        /// direct or indirect dependency relationship run simultaneously;
        /// the scheduler enforces ordering automatically.
        /// 0 = auto-detect CPU count.
        #[arg(short = 'w', long, default_value = "0")]
        workers: usize,

        /// Force-rebuild ALL downstream dependents, not just link dependents
        /// (extends --dependents beyond link-only packages; use together with --dependents
        /// to also include the expansion, or alone to only force-rebuild already-expanded sets)
        #[arg(short = 'R', long)]
        rebuild_dependents: bool,

        /// Force-rebuild ALL upstream dependencies, including already-installed ones
        /// (extends --deps to installed packages; use together with --deps
        /// to also include the expansion, or alone to force-rebuild without expanding)
        #[arg(short = 'D', long)]
        rebuild_dependencies: bool,

        /// Automatically install each package after a successful build
        #[arg(short = 'i', long)]
        install: bool,

        /// Maximum expansion depth for dependency cascade operations (0 = unlimited,
        /// applies to --deps, --dependents, -D, and -R)
        #[arg(long, default_value = "0")]
        depth: usize,

        /// Include the listed packages themselves in the build
        #[arg(short = 's', long = "self")]
        include_self: bool,

        /// Expand build set to include missing upstream dependencies (build + link,
        /// not yet installed; does not include the listed packages themselves)
        #[arg(short = 'd', long = "deps")]
        include_deps: bool,

        /// Expand build set to include packages that link against the target
        /// (does not include the listed packages themselves)
        #[arg(long = "dependents")]
        include_dependents: bool,

        /// Build using the MVP dependency set from [mvp.dependencies] without
        /// requiring a dependency cycle to trigger it
        #[arg(long)]
        mvp: bool,
    },
    /// Validate plan.toml files for syntax and logic errors
    Check {
        /// Plans to check
        targets: Vec<String>,
    },
    /// Download sources for plans without building
    Fetch {
        /// Plans to fetch
        targets: Vec<String>,
    },
    /// Analyze build-time dependency tree from hold tree
    Deps {
        /// Target plan name
        target: String,

        /// Maximum depth to display
        #[arg(long, short, default_value = "0")]
        depth: usize,
    },
    /// Compute and update SHA256 checksums in plan.toml
    Checksum {
        /// Plans to checksum
        targets: Vec<String>,
    },
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
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        // Default/quiet: clean format without timestamps or module paths
        tracing_subscriber::fmt()
            .without_time()
            .with_target(false)
            .with_level(true)
            .with_env_filter(filter)
            .init();
    }
    let config = GlobalConfig::load(cli.config.as_deref())
        .context("failed to load config")?;

    match cli.command {
        Commands::Run {
            targets, until, only, clean, force, workers,
            rebuild_dependents, rebuild_dependencies, install, depth,
            include_self, include_deps, include_dependents, mvp,
        } => {
            Ok(orchestrator::run_build(&config, targets, BuildOptions {
                stage: until, only, clean, force, workers,
                rebuild_dependents, rebuild_dependencies, install, depth: Some(depth),
                checksum: false,
                lint: false,
                verbose: cli.verbose > 0,
                quiet: cli.quiet,
                include_self,
                include_deps,
                include_dependents,
                mvp,
                nproc_per_worker: None, // computed by the scheduler in execute_builds
            })?)
        }
        Commands::Check { targets } => {
            Ok(orchestrator::run_build(&config, targets, BuildOptions {
                lint: true,
                ..Default::default()
            })?)
        }
        Commands::Fetch { targets } => {
            Ok(orchestrator::run_build(&config, targets, BuildOptions {
                stage: Some("extract".to_string()),
                ..Default::default()
            })?)
        }
        Commands::Checksum { targets } => {
            Ok(orchestrator::run_build(&config, targets, BuildOptions {
                checksum: true,
                ..Default::default()
            })?)
        }
        Commands::Deps { target, depth } => {
            // Static analysis of plans in hold tree
            let resolver = wright::builder::orchestrator::setup_resolver(&config)?;
            let all_plans = resolver.get_all_plans()?;

            println!("Plan dependency tree for: {}", target);
            print_plan_tree(&target, &all_plans, "", 1, if depth == 0 { usize::MAX } else { depth })
        }
    }
}

fn print_plan_tree(
    name: &str, 
    all_plans: &HashMap<String, PathBuf>, 
    prefix: &str, 
    current_depth: usize, 
    max_depth: usize
) -> Result<()> {
    if current_depth > max_depth { return Ok(()); }

    let path = all_plans.get(name)
        .ok_or_else(|| anyhow::anyhow!("Plan '{}' not found in hold tree", name))?;
    
    let manifest = PackageManifest::from_file(path)?;
    
    let mut all_deps = Vec::new();
    all_deps.extend(manifest.dependencies.build.iter().cloned());
    all_deps.extend(manifest.dependencies.link.iter().cloned());
    all_deps.extend(manifest.dependencies.runtime.iter().cloned());

    for (i, dep) in all_deps.iter().enumerate() {
        let dep_name = version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None)).0;
        let is_last = i == all_deps.len() - 1;
        let connector = if is_last { "└── " } else { "├── " };
        
        println!("{}{}{}", prefix, connector, dep);
        
        if all_plans.contains_key(&dep_name) {
            let new_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
            print_plan_tree(&dep_name, all_plans, &new_prefix, current_depth + 1, max_depth)?;
        }
    }

    Ok(())
}
