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
use crate::database::Database;
use crate::package::archive;
use crate::package::manifest::PackageManifest;
use crate::package::version;
use crate::repo::source::SimpleResolver;

/// Options for a build run.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub stage: Option<String>,
    pub only: Option<String>,
    pub clean: bool,
    pub lint: bool,
    pub force: bool,
    pub update: bool,
    pub jobs: usize,
    pub rebuild_dependents: bool,
    pub install: bool,
}

/// Run a multi-target build with dependency ordering and parallel execution.
pub fn run_build(config: &GlobalConfig, targets: Vec<String>, opts: BuildOptions) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let mut plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(anyhow::anyhow!("No targets specified to build."));
    }

    // Use a scoped block to ensure the database handle (and its flock) is released 
    // before we start the parallel build/install process.
    {
        let db_path = config.general.db_path.clone();
        let db = Database::open(&db_path).context("failed to open database for dependency resolution")?;
        
        // 1b. Recursive upward expansion: Find missing build/link dependencies in hold tree
        expand_missing_dependencies(&mut plans_to_build, &all_plans, &db)?;
    } 

    // 1c. Transitive downward expansion (ABI rebuilds)
    let reasons = expand_rebuild_deps(&mut plans_to_build, &all_plans, opts.rebuild_dependents)?;

    // 2. Build dependency map
    let graph = build_dep_map(&plans_to_build, opts.update, reasons)?;

    // --- Build Plan Summary ---
    println!("Construction Plan:");
    let mut sorted_targets: Vec<_> = graph.build_set.iter().collect();
    sorted_targets.sort();
    for name in sorted_targets {
        let reason_str = match graph.rebuild_reasons.get(name) {
            Some(RebuildReason::Explicit) => "[NEW]".to_string(),
            Some(RebuildReason::LinkDependency) => "[LINK-REBUILD]".to_string(),
            Some(RebuildReason::Transitive) => "[REV-REBUILD]".to_string(),
            None => "".to_string(),
        };
        println!("  {: <15} {}", reason_str, name);
    }
    println!();

    // 3. Execute builds
    execute_builds(
        config,
        &graph.name_to_path,
        &graph.deps_map,
        &graph.build_set,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    Explicit,
    LinkDependency,
    Transitive,
}

struct PlanGraph {
    name_to_path: HashMap<String, PathBuf>,
    deps_map: HashMap<String, Vec<String>>,
    build_set: HashSet<String>,
    rebuild_reasons: HashMap<String, RebuildReason>,
}

// ---------------------------------------------------------------------------
// Missing dependency expansion (Upward)
// ---------------------------------------------------------------------------

fn expand_missing_dependencies(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
) -> Result<()> {
    let mut build_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PackageManifest::from_file(path) {
            build_set.insert(m.plan.name.clone());
        }
    }

    loop {
        let mut added_any = false;
        let mut to_add_paths = Vec::new();

        for path in plans_to_build.iter() {
            let manifest = PackageManifest::from_file(path)?;
            
            // Check build and link dependencies
            let deps_to_check = manifest.dependencies.build.iter()
                .chain(manifest.dependencies.link.iter());

            for dep in deps_to_check {
                let dep_name = version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None)).0;

                // If not in build set AND not installed, try to find plan
                if !build_set.contains(&dep_name) {
                    if db.get_package(&dep_name)?.is_none() {
                        if let Some(plan_path) = all_plans.get(&dep_name) {
                            info!("Auto-resolving missing dependency: {}", dep_name);
                            to_add_paths.push(plan_path.clone());
                            build_set.insert(dep_name.clone());
                            added_any = true;
                        }
                    }
                }
            }
        }

        for p in to_add_paths {
            plans_to_build.insert(p);
        }

        if !added_any {
            break;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Transitive rebuild expansion (Downward)
// ---------------------------------------------------------------------------

fn expand_rebuild_deps(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    rebuild_all: bool,
) -> Result<HashMap<String, RebuildReason>> {
    let mut reasons = HashMap::new();

    // 1. Build dependency maps for all known plans
    let mut build_runtime_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut link_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_name_to_path: HashMap<String, PathBuf> = HashMap::new();

    for (plan_name, plan_path) in all_plans {
        if let Ok(m) = PackageManifest::from_file(plan_path) {
            let br_deps: Vec<String> = m.dependencies.runtime.iter()
                .chain(m.dependencies.build.iter())
                .map(|d| version::parse_dependency(d)
                    .unwrap_or_else(|_| (d.clone(), None)).0)
                .collect();
            let l_deps: Vec<String> = m.dependencies.link.iter()
                .map(|d| version::parse_dependency(d)
                    .unwrap_or_else(|_| (d.clone(), None)).0)
                .collect();

            build_runtime_deps.insert(plan_name.clone(), br_deps);
            link_deps.insert(plan_name.clone(), l_deps);
            all_name_to_path.insert(plan_name.clone(), plan_path.clone());
        }
    }

    // 2. Initial rebuild set
    let mut rebuild_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PackageManifest::from_file(path) {
            let name = m.plan.name.clone();
            rebuild_set.insert(name.clone());
            reasons.insert(name, RebuildReason::Explicit);
        }
    }

    // 3. Transitively expand
    loop {
        let mut added = false;
        for (name, path) in &all_name_to_path {
            if rebuild_set.contains(name) {
                continue;
            }

            let link_changed = link_deps.get(name).map_or(false, |deps| {
                deps.iter().any(|d| rebuild_set.contains(d))
            });

            let other_changed = rebuild_all && build_runtime_deps.get(name).map_or(false, |deps| {
                deps.iter().any(|d| rebuild_set.contains(d))
            });

            if link_changed || other_changed {
                rebuild_set.insert(name.clone());
                plans_to_build.insert(path.clone());
                reasons.insert(
                    name.clone(),
                    if link_changed { RebuildReason::LinkDependency } else { RebuildReason::Transitive }
                );
                added = true;
            }
        }
        if !added { break; }
    }

    Ok(reasons)
}

// ... (build_dep_map will need to take reasons)


fn build_dep_map(
    plans_to_build: &HashSet<PathBuf>,
    update: bool,
    rebuild_reasons: HashMap<String, RebuildReason>,
) -> Result<PlanGraph> {
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
            for dep in &manifest.dependencies.link {
                deps.push(version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None)).0);
            }
        }
        deps_map.insert(name, deps);
    }

    Ok(PlanGraph { name_to_path, deps_map, build_set, rebuild_reasons })
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
    let (tx, rx) = mpsc::channel::<std::result::Result<String, (String, anyhow::Error)>>();
    let completed = Arc::new(Mutex::new(HashSet::<String>::new()));
    let in_progress = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_count = Arc::new(Mutex::new(0usize));

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());
    let install_lock = Arc::new(Mutex::new(())); // Serializes installation

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
            let opts_clone = opts.clone();
            let install_lock_clone = install_lock.clone();

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
                    &opts_clone,
                );

                match res {
                    Ok(_) => {
                        // Success! Now install if requested
                        if opts_clone.install {
                            let _guard = install_lock_clone.lock().unwrap();
                            info!("Automatically installing built package: {}", name_clone);
                            
                            let output_dir = config_clone.general.components_dir.clone();
                            let archive_path = output_dir.join(manifest.archive_filename());
                            
                            match Database::open(&config_clone.general.db_path) {
                                Ok(db) => {
                                    if let Err(e) = crate::transaction::install_package(
                                        &db, &archive_path, &PathBuf::from("/"), true // Always force for auto-install
                                    ) {
                                        error!("Build succeeded but automatic installation failed for {}: {:#}", name_clone, e);
                                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                                        return;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to open database for automatic installation: {:#}", e);
                                    tx_clone.send(Err((name_clone, e.into()))).unwrap();
                                    return;
                                }
                            }
                        }
                        tx_clone.send(Ok(name_clone)).unwrap();
                    },
                    Err(e) => {
                        error!("Failed to process {}: {:#}", name_clone, e);
                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
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
    opts: &BuildOptions,
) -> Result<()> {
    if opts.update {
        builder.update_hashes(manifest, manifest_path).context("failed to update hashes")?;
        info!("Updated plan hashes: {}", manifest.plan.name);
        return Ok(());
    }

    if opts.lint {
        println!("valid plan: {} {}-{}", manifest.plan.name, manifest.plan.version, manifest.plan.release);
        for split_name in manifest.split.keys() {
            println!("  split: {}", split_name);
        }
        return Ok(());
    }

    if opts.clean {
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
    if !opts.force && opts.only.is_none() {
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
    let result = builder.build(manifest, &plan_dir, opts.stage.clone(), opts.only.clone())?;

    // Skip archive creation when --only is used for non-package stages
    if opts.only.is_none() || opts.only.as_deref() == Some("package") || opts.only.as_deref() == Some("post_package") {
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
