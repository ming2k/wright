//! Build orchestrator — parallel build scheduling, dependency resolution,
//! and --rebuild-deps expansion.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

use anyhow::{Context, Result};
use tracing::{info, warn, error};

use crate::builder::Builder;
use crate::config::{GlobalConfig, AssembliesConfig};
use crate::package::archive;
use crate::package::manifest::PackageManifest;
use crate::package::version;
use crate::repo::source::SimpleResolver;

/// Options for a build run.
pub struct BuildOptions {
    pub stage: Option<String>,
    pub only: Option<String>,
    pub clean: bool,
    pub lint: bool,
    pub force: bool,
    pub update: bool,
    pub jobs: usize,
    pub rebuild_deps: bool,
}

/// Run a multi-target build with dependency ordering and parallel execution.
pub fn run_build(config: &GlobalConfig, targets: Vec<String>, opts: BuildOptions) -> Result<()> {
    let resolver = setup_resolver(config)?;

    // 1. Resolve all targets into manifest paths
    let all_plans = resolver.get_all_plans()?;
    let mut plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(anyhow::anyhow!("No targets specified to build."));
    }

    // 1b. --rebuild-deps: transitively expand with all reverse dependents
    if opts.rebuild_deps {
        expand_rebuild_deps(&mut plans_to_build, &all_plans)?;
    }

    // 2. Build dependency map
    let (name_to_path, deps_map, build_set) = build_dep_map(&plans_to_build, opts.update)?;

    // 3. Execute builds
    execute_builds(
        config,
        &name_to_path,
        &deps_map,
        &build_set,
        &opts,
    )
}

// ---------------------------------------------------------------------------
// Resolver setup
// ---------------------------------------------------------------------------

fn setup_resolver(config: &GlobalConfig) -> Result<SimpleResolver> {
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

    let mut resolver = SimpleResolver::new(config.general.cache_dir.clone());
    resolver.download_timeout = config.network.download_timeout;
    resolver.load_assemblies(all_assemblies);
    resolver.add_plans_dir(config.general.plans_dir.clone());
    resolver.add_plans_dir(PathBuf::from("../wright-dockyard/plans"));
    resolver.add_plans_dir(PathBuf::from("../plans"));
    resolver.add_plans_dir(PathBuf::from("./plans"));

    Ok(resolver)
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

fn resolve_targets(
    targets: &[String],
    all_plans: &HashMap<String, PathBuf>,
    resolver: &SimpleResolver,
) -> Result<HashSet<PathBuf>> {
    let mut plans_to_build = HashSet::new();

    for target in targets {
        let clean_target = target.trim();
        if clean_target.is_empty() { continue; }

        if let Some(assembly_name) = clean_target.strip_prefix('@') {
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
                let mut found = false;
                for plans_dir in &resolver.plans_dirs {
                    let candidate = plans_dir.join(clean_target).join("plan.toml");
                    if candidate.exists() {
                        PackageManifest::from_file(&candidate)
                            .context(format!("failed to parse plan '{}'", clean_target))?;
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

    Ok(plans_to_build)
}

// ---------------------------------------------------------------------------
// --rebuild-deps expansion
// ---------------------------------------------------------------------------

fn expand_rebuild_deps(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
) -> Result<()> {
    // Build a name→path + name→deps map for all known plans
    let mut all_plan_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_name_to_path: HashMap<String, PathBuf> = HashMap::new();
    for (plan_name, plan_path) in all_plans {
        if let Ok(m) = PackageManifest::from_file(plan_path) {
            let deps: Vec<String> = m.dependencies.runtime.iter()
                .chain(m.dependencies.build.iter())
                .map(|d| version::parse_dependency(d)
                    .unwrap_or_else(|_| (d.clone(), None)).0)
                .collect();
            all_plan_deps.insert(plan_name.clone(), deps);
            all_name_to_path.insert(plan_name.clone(), plan_path.clone());
        }
    }

    // Collect original target names
    let mut rebuild_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PackageManifest::from_file(path) {
            rebuild_set.insert(m.plan.name.clone());
        }
    }

    // Iterate until stable
    loop {
        let mut added = false;
        for (name, deps) in &all_plan_deps {
            if rebuild_set.contains(name) {
                continue;
            }
            if deps.iter().any(|d| rebuild_set.contains(d)) {
                rebuild_set.insert(name.clone());
                added = true;
            }
        }
        if !added { break; }
    }

    // Add newly discovered plans to the build set
    for name in &rebuild_set {
        if let Some(path) = all_name_to_path.get(name) {
            if plans_to_build.insert(path.clone()) {
                info!("--rebuild-deps: including {}", name);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Dependency map construction
// ---------------------------------------------------------------------------

fn build_dep_map(
    plans_to_build: &HashSet<PathBuf>,
    update: bool,
) -> Result<(HashMap<String, PathBuf>, HashMap<String, Vec<String>>, HashSet<String>)> {
    let mut name_to_path = HashMap::new();
    let mut deps_map = HashMap::new();
    let mut build_set = HashSet::new();

    for path in plans_to_build {
        let manifest = PackageManifest::from_file(path)?;
        let name = manifest.plan.name.clone();
        name_to_path.insert(name.clone(), path.clone());
        build_set.insert(name.clone());

        let mut deps = Vec::new();
        if !update {
            for dep in &manifest.dependencies.build {
                deps.push(version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None)).0);
            }
            for dep in &manifest.dependencies.runtime {
                deps.push(version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None)).0);
            }
        }
        deps_map.insert(name, deps);
    }

    Ok((name_to_path, deps_map, build_set))
}

// ---------------------------------------------------------------------------
// Parallel build execution
// ---------------------------------------------------------------------------

fn execute_builds(
    config: &GlobalConfig,
    name_to_path: &HashMap<String, PathBuf>,
    deps_map: &HashMap<String, Vec<String>>,
    build_set: &HashSet<String>,
    opts: &BuildOptions,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let completed = Arc::new(Mutex::new(HashSet::<String>::new()));
    let in_progress = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_count = Arc::new(Mutex::new(0usize));

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let all_deps_met = opts.update || deps_map.get(name).unwrap().iter()
                        .filter(|d| build_set.contains(*d))
                        .all(|d| comp.contains(d));

                    if all_deps_met {
                        ready_to_launch.push(name.clone());
                    }
                }
            }
        }

        for name in ready_to_launch {
            if in_progress.lock().unwrap().len() >= opts.jobs {
                break;
            }

            in_progress.lock().unwrap().insert(name.clone());

            let tx_clone = tx.clone();
            let name_clone = name.clone();
            let path = name_to_path.get(&name).unwrap().clone();
            let builder_clone = builder.clone();
            let config_clone = config_arc.clone();
            let stage_clone = opts.stage.clone();
            let only_clone = opts.only.clone();
            let clean = opts.clean;
            let lint = opts.lint;
            let force = opts.force;
            let update = opts.update;

            std::thread::spawn(move || {
                let manifest = match PackageManifest::from_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                        return;
                    }
                };
                let res = build_one(
                    &builder_clone, &manifest, &path, &config_clone,
                    stage_clone.as_deref(), only_clone.as_deref(),
                    clean, lint, force, update,
                );

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
                if !opts.update {
                    return Err(anyhow::anyhow!("Construction failed due to error in {}", name));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().unwrap();
    let final_completed = completed.lock().unwrap().len();

    if final_failed > 0 {
        warn!("Construction finished with {} successes and {} failures.", final_completed, final_failed);
        if !opts.update {
            return Err(anyhow::anyhow!("Some parts failed to manufacture."));
        }
    } else {
        info!("All {} tasks completed successfully.", final_completed);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Single package build
// ---------------------------------------------------------------------------

fn build_one(
    builder: &Builder,
    manifest: &PackageManifest,
    manifest_path: &Path,
    config: &GlobalConfig,
    stage: Option<&str>,
    only: Option<&str>,
    clean: bool,
    lint: bool,
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

    if clean {
        builder.clean(manifest).context("failed to clean workspace")?;
    }

    let output_dir = if config.general.components_dir.exists()
        || std::fs::create_dir_all(&config.general.components_dir).is_ok()
    {
        config.general.components_dir.clone()
    } else {
        std::env::current_dir()?
    };

    // Skip if archive already exists (unless --force or --only)
    if !force && only.is_none() {
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
    let result = builder.build(manifest, &plan_dir, stage.map(String::from), only.map(String::from))?;

    // Skip archive creation when --only is used for non-package stages
    if only.is_none() || only == Some("package") || only == Some("post_package") {
        let archive_path = archive::create_archive(&result.pkg_dir, manifest, &output_dir)?;
        info!("Part stored in the Components Hold: {}", archive_path.display());

        for (split_name, split_pkg) in &manifest.split {
            let split_pkg_dir = result.split_pkg_dirs.get(split_name)
                .ok_or_else(|| anyhow::anyhow!("missing split pkg_dir for '{}'", split_name))?;
            let split_manifest = split_pkg.to_manifest(split_name, manifest);
            let split_archive = archive::create_archive(split_pkg_dir, &split_manifest, &output_dir)?;
            info!("Split part stored: {}", split_archive.display());
        }
    }

    Ok(())
}
