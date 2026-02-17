use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{info, warn, error};

use wright::config::{GlobalConfig, AssembliesConfig};
use wright::database::Database;
use wright::package::manifest::PackageManifest;
use wright::package::archive;
use wright::builder::Builder;
use wright::transaction;

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
    },
    /// List installed packages
    List {
        /// Show only installed packages (default)
        #[arg(long)]
        installed: bool,
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
    },
    /// Build packages from plan.toml files
    Build {
        /// Paths to plan directories, part names, or @assemblies
        targets: Vec<String>,

        /// Stop after a specific lifecycle stage
        #[arg(long)]
        stage: Option<String>,

        /// Clean build directory before building
        #[arg(long)]
        clean: bool,

        /// Validate plan syntax only
        #[arg(long)]
        lint: bool,

        /// Force rebuild even if part exists
        #[arg(long)]
        rebuild: bool,

        /// Force overwrite existing archive (skip check disabled)
        #[arg(long, short)]
        force: bool,

        /// Update sha256 checksums in plan
        #[arg(long)]
        update: bool,

        /// Max number of parallel builds
        #[arg(short = 'j', long, default_value = "1")]
        jobs: usize,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let config = GlobalConfig::load(cli.config.as_deref())
        .context("failed to load config")?;

    // Build subcommand has its own setup path — handle it separately
    if let Commands::Build { targets, stage, clean, lint, rebuild, force, update, jobs } = cli.command {
        return run_build(&config, targets, stage, clean, lint, rebuild, force, update, jobs);
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
        Commands::Remove { packages } => {
            for name in &packages {
                match transaction::remove_package(&db, name, &root_dir) {
                    Ok(()) => println!("removed: {}", name),
                    Err(e) => {
                        eprintln!("error removing {}: {}", name, e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::List { .. } => {
            let packages = db.list_packages()
                .context("failed to list packages")?;
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
        Commands::Verify { package } => {
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

fn run_build(
    config: &GlobalConfig,
    targets: Vec<String>,
    stage: Option<String>,
    clean: bool,
    lint: bool,
    rebuild: bool,
    force: bool,
    update: bool,
    jobs: usize,
) -> Result<()> {
    // Load all assemblies from multiple locations
    let mut all_assemblies = AssembliesConfig { assemblies: HashMap::new() };

    if let Ok(f) = AssembliesConfig::load_all(&config.general.assemblies_dir) {
        all_assemblies.assemblies.extend(f.assemblies);
    }
    if let Ok(f) = AssembliesConfig::load_all(&config.general.plans_dir.join("assemblies")) {
        all_assemblies.assemblies.extend(f.assemblies);
    }
    if let Ok(f) = AssembliesConfig::load_all(Path::new("./assemblies")) {
        all_assemblies.assemblies.extend(f.assemblies);
    }
    if let Ok(f) = AssembliesConfig::load_all(Path::new("../wright-dockyard/assemblies")) {
        all_assemblies.assemblies.extend(f.assemblies);
    }

    let mut resolver = wright::repo::source::SimpleResolver::new(config.general.cache_dir.clone());
    resolver.download_timeout = config.network.download_timeout;
    resolver.load_assemblies(all_assemblies);
    resolver.add_plans_dir(config.general.plans_dir.clone());
    resolver.add_plans_dir(PathBuf::from("../wright-dockyard/plans"));
    resolver.add_plans_dir(PathBuf::from("../plans"));
    resolver.add_plans_dir(PathBuf::from("./plans"));

    // 1. Resolve all targets into manifest paths
    let mut plans_to_build = HashSet::new();
    let all_plans = resolver.get_all_plans()?;

    for target in &targets {
        let clean_target = target.trim();
        if clean_target.is_empty() { continue; }

        if clean_target.starts_with('@') {
            let assembly_name = &clean_target[1..];
            let paths = resolver.resolve_assembly(assembly_name)?;
            if paths.is_empty() {
                warn!("Assembly not found: {}", assembly_name);
            }
            for p in paths { plans_to_build.insert(p); }
        } else if let Some(path) = all_plans.get(clean_target) {
            plans_to_build.insert(path.clone());
        } else {
            let plan_path = PathBuf::from(clean_target);
            let manifest_path = if plan_path.is_file() {
                plan_path
            } else {
                plan_path.join("plan.toml")
            };

            if manifest_path.exists() {
                plans_to_build.insert(manifest_path);
            } else {
                // The plan name wasn't in get_all_plans() — it may exist
                // but have a syntax error (get_all_plans silently skips
                // invalid manifests). Search plan directories for a
                // matching directory and try to parse it, surfacing the
                // real error.
                let mut found = false;
                for plans_dir in &resolver.plans_dirs {
                    let candidate = plans_dir.join(clean_target).join("plan.toml");
                    if candidate.exists() {
                        // Try to parse — this will produce the real error
                        PackageManifest::from_file(&candidate)
                            .context(format!("failed to parse plan '{}'", clean_target))?;
                        // If parsing succeeds (shouldn't normally reach here), use it
                        plans_to_build.insert(candidate);
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(anyhow::anyhow!("Target not found: {}", clean_target));
                }
            }
        }
    }

    if plans_to_build.is_empty() {
        return Err(anyhow::anyhow!("No targets specified to build."));
    }

    // 2. Build Dependency Map
    let mut name_to_path = HashMap::new();
    let mut deps_map = HashMap::new();
    let mut build_set = HashSet::new();

    for path in &plans_to_build {
        let manifest = PackageManifest::from_file(path)?;
        let name = manifest.plan.name.clone();
        name_to_path.insert(name.clone(), path.clone());
        build_set.insert(name.clone());

        let mut deps = Vec::new();
        if !update {
            for dep in &manifest.dependencies.build {
                 deps.push(wright::package::version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None)).0);
            }
            for dep in &manifest.dependencies.runtime {
                 deps.push(wright::package::version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None)).0);
            }
        }
        deps_map.insert(name, deps);
    }

    // 3. Build Orchestrator
    let (tx, rx) = mpsc::channel();
    let completed = Arc::new(Mutex::new(HashSet::new()));
    let in_progress = Arc::new(Mutex::new(HashSet::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::new()));
    let failed_count = Arc::new(Mutex::new(0));

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

            for name in &build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    // Update mode ignores dependencies
                    let all_deps_met = update || deps_map.get(name).unwrap().iter()
                        .filter(|d| build_set.contains(*d))
                        .all(|d| comp.contains(d));

                    if all_deps_met {
                        ready_to_launch.push(name.clone());
                    }
                }
            }
        }

        for name in ready_to_launch {
            if in_progress.lock().unwrap().len() >= jobs {
                break;
            }

            in_progress.lock().unwrap().insert(name.clone());

            let tx_clone = tx.clone();
            let name_clone = name.clone();
            let path = name_to_path.get(&name).unwrap().clone();
            let builder_clone = builder.clone();
            let config_clone = config_arc.clone();
            let stage_clone = stage.clone();

            std::thread::spawn(move || {
                let manifest = match PackageManifest::from_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                        return;
                    }
                };
                let res = build_one(&builder_clone, &manifest, &path, &config_clone,
                    stage_clone.as_deref(), clean, lint, rebuild, force, update);

                match res {
                    Ok(_) => tx_clone.send(Ok(name_clone)).unwrap(),
                    Err(e) => {
                        error!("Failed to process {}: {:#}", name_clone, e);
                        tx_clone.send(Err((name_clone, e))).unwrap();
                    }
                }
            });
        }

        let finished_count = completed.lock().unwrap().len() + *failed_count.lock().unwrap();
        if in_progress.lock().unwrap().is_empty() && finished_count == build_set.len() {
            break;
        }

        // Deadlock detection
        if in_progress.lock().unwrap().is_empty() && finished_count < build_set.len() {
             return Err(anyhow::anyhow!("Deadlock detected or dependency missing from plan set"));
        }

        match rx.recv().unwrap() {
            Ok(name) => {
                in_progress.lock().unwrap().remove(&name);
                completed.lock().unwrap().insert(name);
            }
            Err((name, _)) => {
                in_progress.lock().unwrap().remove(&name);
                failed_set.lock().unwrap().insert(name.clone());
                *failed_count.lock().unwrap() += 1;
                // In update mode, we continue. In build mode, we stop.
                if !update {
                    return Err(anyhow::anyhow!("Construction failed due to error in {}", name));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().unwrap();
    let final_completed = completed.lock().unwrap().len();

    if final_failed > 0 {
        warn!("Construction finished with {} successes and {} failures.", final_completed, final_failed);
        if !update {
            return Err(anyhow::anyhow!("Some parts failed to manufacture."));
        }
    } else {
        info!("All {} tasks completed successfully.", final_completed);
    }

    Ok(())
}

fn build_one(
    builder: &Builder,
    manifest: &PackageManifest,
    manifest_path: &Path,
    config: &GlobalConfig,
    stage: Option<&str>,
    clean: bool,
    lint: bool,
    rebuild: bool,
    force: bool,
    update: bool,
) -> Result<()> {
    if update {
        builder.update_hashes(manifest, manifest_path).context("failed to update hashes")?;
        info!("Updated plan hashes: {}", manifest.plan.name);
        return Ok(());
    }

    if lint {
        println!("valid plan: {} {}-{}", manifest.plan.name, manifest.plan.version, manifest.plan.release);
        for split_name in manifest.split.keys() {
            println!("  split: {}", split_name);
        }
        return Ok(());
    }

    if clean || rebuild {
        builder.clean(manifest).context("failed to clean workspace")?;
    }

    let output_dir = if config.general.components_dir.exists() || std::fs::create_dir_all(&config.general.components_dir).is_ok() {
        config.general.components_dir.clone()
    } else {
        std::env::current_dir()?
    };

    // Skip if archive already exists (unless --force or --rebuild)
    if !force && !rebuild {
        let archive_name = manifest.archive_filename();
        let existing = output_dir.join(&archive_name);
        let all_exist = existing.exists() && manifest.split.iter().all(|(split_name, split_pkg)| {
            let split_manifest = split_pkg.to_manifest(split_name, manifest);
            output_dir.join(split_manifest.archive_filename()).exists()
        });
        if all_exist && existing.exists() {
            info!("Skipping {} (all archives already exist, use --force to rebuild)", manifest.plan.name);
            return Ok(());
        }
    }

    info!("Manufacturing part {}...", manifest.plan.name);
    let plan_dir = manifest_path.parent().unwrap().to_path_buf();
    let result = builder.build(manifest, &plan_dir, stage.map(String::from))?;

    let archive_path = archive::create_archive(&result.pkg_dir, manifest, &output_dir)?;
    info!("Part stored in the Components Hold: {}", archive_path.display());

    // Create split package archives
    for (split_name, split_pkg) in &manifest.split {
        let split_pkg_dir = result.split_pkg_dirs.get(split_name)
            .ok_or_else(|| anyhow::anyhow!("missing split pkg_dir for '{}'", split_name))?;
        let split_manifest = split_pkg.to_manifest(split_name, manifest);
        let split_archive = archive::create_archive(split_pkg_dir, &split_manifest, &output_dir)?;
        info!("Split part stored: {}", split_archive.display());
    }

    Ok(())
}
