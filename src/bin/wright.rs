use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use wright::config::GlobalConfig;
use wright::database::Database;
use wright::builder::orchestrator::{self, BuildOptions};
use wright::transaction;
use wright::query;

#[derive(Parser)]
#[command(name = "wright", about = "wright package manager")]
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
}

#[derive(Subcommand)]
enum Commands {
    /// Install packages from local .wright.tar.zst files
    Install {
        /// Package archive files to install
        #[arg(required = true)]
        packages: Vec<PathBuf>,

        /// Force reinstall even if already installed
        #[arg(long)]
        force: bool,

        /// Skip dependency resolution
        #[arg(long)]
        nodeps: bool,
    },
    /// Upgrade installed packages from local .wright.tar.zst files
    Upgrade {
        /// Package archive files to upgrade
        #[arg(required = true)]
        packages: Vec<PathBuf>,

        /// Force upgrade even if version is not newer
        #[arg(long)]
        force: bool,
    },
    /// Remove installed packages
    Remove {
        /// Package names to remove
        #[arg(required = true)]
        packages: Vec<String>,

        /// Force removal even if other packages depend on this one
        #[arg(long)]
        force: bool,

        /// Recursively remove all packages that depend on the target
        #[arg(long, short)]
        recursive: bool,
    },
    /// Analyze package dependency relationships
    Deps {
        /// Package name
        package: Option<String>,

        /// Show reverse dependencies (what depends on this package)
        #[arg(long, short)]
        reverse: bool,

        /// Maximum depth to display (0 = unlimited)
        #[arg(long, short, default_value = "0")]
        depth: usize,

        /// Filter output to only show matching package names
        #[arg(long, short)]
        filter: Option<String>,

        /// Show full system dependency tree
        #[arg(long, short)]
        tree: bool,
    },
    /// List installed packages
    List {
        /// Show only installed packages (default)
        #[arg(long)]
        installed: bool,

        /// Show only top-level (root) packages
        #[arg(long, short)]
        roots: bool,
    },
    /// Show detailed package information
    Query {
        /// Package name
        package: String,
    },
    /// Search installed packages by keyword
    Search {
        /// Search keyword
        keyword: String,
    },
    /// List files owned by a package
    Files {
        /// Package name
        package: String,
    },
    /// Find which package owns a file
    Owner {
        /// File path
        file: String,
    },
    /// Verify installed package file integrity
    Verify {
        /// Package name (or all if omitted)
        package: Option<String>,

        /// Check for broken dependencies system-wide
        #[arg(long)]
        check_deps: bool,
    },
    /// Build packages from plan.toml files
    Build {
        /// Paths to plan directories, part names, or @assemblies
        targets: Vec<String>,

        /// Stop after a specific lifecycle stage
        #[arg(long)]
        stage: Option<String>,

        /// Run only a single lifecycle stage (preserve build dir from previous build)
        #[arg(long)]
        only: Option<String>,

        /// Clean build directory before building
        #[arg(long)]
        clean: bool,

        /// Validate plan syntax only
        #[arg(long)]
        lint: bool,

        /// Force overwrite existing archive
        #[arg(long, short)]
        force: bool,

        /// Update sha256 checksums in plan
        #[arg(long)]
        update: bool,

        /// Max number of parallel builds
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,

        /// Rebuild all packages that depend on the target (for ABI breakage)
        #[arg(short = 'R', long)]
        rebuild_dependents: bool,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let config = GlobalConfig::load(cli.config.as_deref())
        .context("failed to load config")?;

    // Build subcommand has its own setup path
    if let Commands::Build { targets, stage, only, clean, lint, force, update, jobs, rebuild_dependents } = cli.command {
        return orchestrator::run_build(&config, targets, BuildOptions {
            stage, only, clean, lint, force, update, jobs, rebuild_dependents,
        });
    }

    let repo_config = wright::config::RepoConfig::load(None)
        .context("failed to load repo config")?;

    let db_path = cli.db.unwrap_or(config.general.db_path.clone());
    let root_dir = cli.root.unwrap_or_else(|| PathBuf::from("/"));

    let db = Database::open(&db_path)
        .context("failed to open database")?;

    let mut resolver = wright::repo::source::SimpleResolver::new(config.general.cache_dir.join("packages"));
    resolver.load_from_config(&repo_config);
    resolver.add_search_dir(config.general.cache_dir.join("packages"));
    resolver.add_search_dir(config.general.components_dir.clone());
    resolver.add_search_dir(std::env::current_dir()?);
    resolver.add_plans_dir(config.general.plans_dir.clone());

    match cli.command {
        Commands::Install { packages, force, nodeps } => {
            let pkg_paths: Vec<PathBuf> = packages.iter().map(|p| {
                if p.exists() {
                    p.clone()
                } else {
                    std::env::current_dir().unwrap().join(p)
                }
            }).collect();

            for path in &pkg_paths {
                if !path.exists() {
                    eprintln!("error: file not found: {}", path.display());
                    std::process::exit(1);
                }
            }

            match transaction::install_packages(&db, &pkg_paths, &root_dir, &resolver, force, nodeps) {
                Ok(()) => println!("installation completed successfully"),
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Upgrade { packages, force } => {
            let pkg_paths: Vec<PathBuf> = packages.iter().map(|p| {
                if p.exists() {
                    p.clone()
                } else {
                    std::env::current_dir().unwrap().join(p)
                }
            }).collect();

            for path in &pkg_paths {
                if !path.exists() {
                    eprintln!("error: file not found: {}", path.display());
                    std::process::exit(1);
                }
            }

            for path in &pkg_paths {
                match transaction::upgrade_package(&db, path, &root_dir, force) {
                    Ok(()) => println!("upgraded: {}", path.display()),
                    Err(e) => {
                        eprintln!("error upgrading {}: {}", path.display(), e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Remove { packages, force, recursive } => {
            for name in &packages {
                if recursive {
                    let dependents = db.get_recursive_dependents(name)
                        .context(format!("failed to resolve dependents of {}", name))?;

                    if !dependents.is_empty() {
                        println!("will also remove (depends on {}): {}", name, dependents.join(", "));
                    }

                    for dep in &dependents {
                        match transaction::remove_package(&db, dep, &root_dir, true) {
                            Ok(()) => println!("removed: {}", dep),
                            Err(e) => {
                                eprintln!("error removing {}: {}", dep, e);
                                std::process::exit(1);
                            }
                        }
                    }
                }

                match transaction::remove_package(&db, name, &root_dir, force || recursive) {
                    Ok(()) => println!("removed: {}", name),
                    Err(e) => {
                        eprintln!("error removing {}: {}", name, e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Deps { package, reverse, depth, filter, tree } => {
            if tree {
                query::print_system_tree(&db)?;
                return Ok(());
            }

            let package_name = package.ok_or_else(|| {
                anyhow::anyhow!("package name is required unless using --tree")
            })?;

            let pkg = db.get_package(&package_name)
                .context("failed to query package")?;
            if pkg.is_none() {
                eprintln!("package '{}' is not installed", package_name);
                std::process::exit(1);
            }

            let max_depth = if depth == 0 { usize::MAX } else { depth };

            println!("{}", package_name);
            if reverse {
                query::print_reverse_dep_tree(&db, &package_name, "", 1, max_depth, filter.as_deref())?;
            } else {
                query::print_dep_tree(&db, &package_name, "", 1, max_depth, filter.as_deref())?;
            }
        }
        Commands::List { roots, .. } => {
            let packages = if roots {
                db.get_root_packages()
            } else {
                db.list_packages()
            }.context("failed to list packages")?;

            if packages.is_empty() {
                println!("no packages installed");
            } else {
                for pkg in &packages {
                    println!("{} {}-{} ({})",
                        pkg.name, pkg.version, pkg.release, pkg.arch);
                }
            }
        }
        Commands::Query { package } => {
            let pkg = db.get_package(&package)
                .context("failed to query package")?;
            match pkg {
                Some(info) => {
                    println!("Name        : {}", info.name);
                    println!("Version     : {}", info.version);
                    println!("Release     : {}", info.release);
                    println!("Description : {}", info.description);
                    println!("Architecture: {}", info.arch);
                    println!("License     : {}", info.license);
                    if let Some(ref url) = info.url {
                        println!("URL         : {}", url);
                    }
                    println!("Install Size: {} bytes", info.install_size);
                    println!("Installed At: {}", info.installed_at);
                    if let Some(ref hash) = info.pkg_hash {
                        println!("Package Hash: {}", hash);
                    }
                }
                None => {
                    eprintln!("package '{}' is not installed", package);
                    std::process::exit(1);
                }
            }
        }
        Commands::Search { keyword } => {
            let results = db.search_packages(&keyword)
                .context("failed to search packages")?;
            if results.is_empty() {
                println!("no packages found matching '{}'", keyword);
            } else {
                for pkg in &results {
                    println!("{} {}-{} - {}",
                        pkg.name, pkg.version, pkg.release, pkg.description);
                }
            }
        }
        Commands::Files { package } => {
            let pkg = db.get_package(&package)
                .context("failed to query package")?;
            match pkg {
                Some(info) => {
                    let files = db.get_files(info.id)
                        .context("failed to get files")?;
                    for file in &files {
                        println!("{}", file.path);
                    }
                }
                None => {
                    eprintln!("package '{}' is not installed", package);
                    std::process::exit(1);
                }
            }
        }
        Commands::Owner { file } => {
            match db.find_owner(&file)
                .context("failed to find owner")? {
                Some(owner) => println!("{} is owned by {}", file, owner),
                None => {
                    println!("{} is not owned by any package", file);
                    std::process::exit(1);
                }
            }
        }
        Commands::Verify { package, check_deps } => {
            if check_deps {
                let broken = query::check_dependencies(&db)?;
                if broken.is_empty() {
                    println!("All dependencies satisfied.");
                } else {
                    for issue in broken {
                        eprintln!("{}", issue);
                    }
                    std::process::exit(1);
                }
                return Ok(());
            }

            let packages_to_verify: Vec<String> = if let Some(name) = package {
                vec![name]
            } else {
                db.list_packages()
                    .context("failed to list packages")?
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            };

            let mut all_ok = true;
            for name in &packages_to_verify {
                let issues = transaction::verify_package(&db, name, &root_dir)
                    .context(format!("failed to verify {}", name))?;
                if issues.is_empty() {
                    println!("{}: OK", name);
                } else {
                    all_ok = false;
                    println!("{}:", name);
                    for issue in &issues {
                        println!("  {}", issue);
                    }
                }
            }
            if !all_ok {
                std::process::exit(1);
            }
        }
        Commands::Build { .. } => unreachable!(),
    }

    Ok(())
}
