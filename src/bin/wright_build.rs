use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::{info, warn, error};

use wright::config::{GlobalConfig, AssembliesConfig};
use wright::package::manifest::PackageManifest;
use wright::package::archive;
use wright::builder::Builder;

#[derive(Parser)]
#[command(name = "wright-build", about = "wright maritime construction tool")]
struct Cli {
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

    /// Update sha256 checksums in plan
    #[arg(long)]
    update: bool,

    /// Path to config file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Max number of parallel builds
    #[arg(short = 'j', long, default_value = "1")]
    jobs: usize,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let config = GlobalConfig::load(cli.config.as_deref())
        .context("failed to load config")?;

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

    for target in &cli.targets {
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
                plan_path.join("package.toml")
            };

            if manifest_path.exists() {
                plans_to_build.insert(manifest_path);
            } else {
                return Err(anyhow::anyhow!("Target not found: {}", clean_target));
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
        let name = manifest.package.name.clone();
        name_to_path.insert(name.clone(), path.clone());
        build_set.insert(name.clone());
        
        let mut deps = Vec::new();
        if !cli.update {
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
    let cli_arc = Arc::new(cli);
    let config_arc = Arc::new(config);

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();
            
            for name in &build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    // Update mode ignores dependencies
                    let all_deps_met = cli_arc.update || deps_map.get(name).unwrap().iter()
                        .filter(|d| build_set.contains(*d))
                        .all(|d| comp.contains(d));
                    
                    if all_deps_met {
                        ready_to_launch.push(name.clone());
                    }
                }
            }
        }

        for name in ready_to_launch {
            if in_progress.lock().unwrap().len() >= cli_arc.jobs {
                break;
            }

            in_progress.lock().unwrap().insert(name.clone());
            
            let tx_clone = tx.clone();
            let name_clone = name.clone();
            let path = name_to_path.get(&name).unwrap().clone();
            let builder_clone = builder.clone();
            let cli_clone = cli_arc.clone();
            let config_clone = config_arc.clone();

            std::thread::spawn(move || {
                let manifest = match PackageManifest::from_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                        return;
                    }
                };
                let res = build_one(&builder_clone, &manifest, &path, &cli_clone, &config_clone);
                
                match res {
                    Ok(_) => tx_clone.send(Ok(name_clone)).unwrap(),
                    Err(e) => {
                        error!("Failed to process {}: {}", name_clone, e);
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
                if !cli_arc.update {
                    return Err(anyhow::anyhow!("Construction failed due to error in {}", name));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().unwrap();
    let final_completed = completed.lock().unwrap().len();
    
    if final_failed > 0 {
        warn!("Construction finished with {} successes and {} failures.", final_completed, final_failed);
        if !cli_arc.update {
            return Err(anyhow::anyhow!("Some parts failed to manufacture."));
        }
    } else {
        info!("All {} tasks completed successfully.", final_completed);
    }

    Ok(())
}

fn build_one(builder: &Builder, manifest: &PackageManifest, manifest_path: &Path, cli: &Cli, config: &GlobalConfig) -> Result<()> {
    if cli.update {
        builder.update_hashes(manifest, manifest_path).context("failed to update hashes")?;
        info!("Updated plan hashes: {}", manifest.package.name);
        return Ok(());
    }

    if cli.lint {
        println!("valid plan: {} {}-{}", manifest.package.name, manifest.package.version, manifest.package.release);
        return Ok(());
    }

    if cli.clean || cli.rebuild {
        builder.clean(manifest).context("failed to clean workspace")?;
    }

    info!("Manufacturing part {}...", manifest.package.name);
    let plan_dir = manifest_path.parent().unwrap().to_path_buf();
    let result = builder.build(manifest, &plan_dir, cli.stage.clone())?;

    let output_dir = if config.general.components_dir.exists() || std::fs::create_dir_all(&config.general.components_dir).is_ok() {
        config.general.components_dir.clone()
    } else {
        std::env::current_dir()?
    };

    let archive_path = archive::create_archive(&result.pkg_dir, manifest, &output_dir)?;
    info!("Part stored in the Components Hold: {}", archive_path.display());
    Ok(())
}
