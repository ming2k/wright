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

        /// Stop after a specific lifecycle stage
        #[arg(long)]
        stage: Option<String>,

        /// Run only a single lifecycle stage
        #[arg(long)]
        only: Option<String>,

        /// Clean build directory before building
        #[arg(long)]
        clean: bool,

        /// Force overwrite existing archive
        #[arg(long, short)]
        force: bool,

        /// Max number of parallel builds (0 = auto-detect)
        #[arg(short = 'j', long, default_value = "0")]
        jobs: usize,

        /// Rebuild all packages that depend on the target (for ABI breakage)
        #[arg(short = 'R', long)]
        rebuild_dependents: bool,

        /// Rebuild all packages that the target depends on
        #[arg(short = 'D', long)]
        rebuild_dependencies: bool,

        /// Automatically install built packages
        #[arg(short = 'i', long)]
        install: bool,

        /// Maximum recursion depth for -D and -R (0 = unlimited)
        #[arg(long, default_value = "0")]
        depth: usize,
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

    let mut filter = if cli.quiet {
        EnvFilter::new("warn")
    } else {
        EnvFilter::new("info")
    };
    if cli.verbose > 0 {
        filter = EnvFilter::new("debug");
    }
    if cli.verbose > 1 {
        filter = EnvFilter::new("trace");
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let config = GlobalConfig::load(cli.config.as_deref())
        .context("failed to load config")?;

    match cli.command {
        Commands::Run { 
            targets, stage, only, clean, force, jobs, 
            rebuild_dependents, rebuild_dependencies, install, depth 
        } => {
            orchestrator::run_build(&config, targets, BuildOptions {
                stage, only, clean, force, jobs,
                rebuild_dependents, rebuild_dependencies, install, depth: Some(depth),
                checksum: false,
                lint: false,
            })
        }
        Commands::Check { targets } => {
            orchestrator::run_build(&config, targets, BuildOptions {
                lint: true,
                ..Default::default()
            })
        }
        Commands::Fetch { targets } => {
            orchestrator::run_build(&config, targets, BuildOptions {
                stage: Some("extract".to_string()),
                ..Default::default()
            })
        }
        Commands::Checksum { targets } => {
            orchestrator::run_build(&config, targets, BuildOptions {
                checksum: true,
                ..Default::default()
            })
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
