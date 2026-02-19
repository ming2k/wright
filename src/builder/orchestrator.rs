//! Build orchestrator — parallel build scheduling, dependency resolution,
//! cascade expansion, and automatic bootstrap cycle detection/resolution.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::mpsc;

use crate::error::{WrightError, Result, WrightResultExt};
use tracing::{info, warn, error};

use crate::builder::Builder;
use crate::config::{GlobalConfig, AssembliesConfig};
use crate::database::Database;
use crate::package::archive;
use crate::package::manifest::PackageManifest;
use crate::package::version;
use crate::repo::source::SimpleResolver;
use crate::package::manifest::PhaseDependencies;

/// Options for a build run.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub stage: Option<String>,
    pub only: Option<String>,
    pub clean: bool,
    pub lint: bool,
    pub force: bool,
    pub checksum: bool,
    pub jobs: usize,
    pub rebuild_dependents: bool,
    pub rebuild_dependencies: bool,
    pub install: bool,
    pub depth: Option<usize>,
    pub verbose: bool,
    pub quiet: bool,
    /// --self (-s): include the listed packages themselves in the build.
    pub include_self: bool,
    /// --deps (-d): include missing upstream dependencies in the build.
    pub include_deps: bool,
    /// --dependents: include downstream link-rebuild dependents (not the listed packages themselves).
    pub include_dependents: bool,
}

impl BuildOptions {
    /// Returns true if this is a real build operation.
    /// Per-plan metadata operations (checksum, lint, fetch) skip all
    /// dependency cascade expansion.
    fn is_build_op(&self) -> bool {
        !self.checksum && !self.lint && self.stage.as_deref() != Some("extract")
    }
}

/// Run a multi-target build with dependency ordering and parallel execution.
pub fn run_build(config: &GlobalConfig, targets: Vec<String>, opts: BuildOptions) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let mut plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::BuildError("No targets specified to build.".to_string()));
    }

    if opts.lint {
        return lint_dependency_graph(&plans_to_build, &all_plans);
    }

    let actual_max = {
        let max_depth = opts.depth.unwrap_or(1);
        if max_depth == 0 { usize::MAX } else { max_depth }
    };

    // Determine effective scope.
    // Default (no explicit scope flags): build self + resolve missing deps, no cascade.
    // When any scope flag is given, only the explicitly requested scopes apply.
    let any_explicit_scope = opts.include_self || opts.include_deps || opts.include_dependents;
    let do_self       = if any_explicit_scope { opts.include_self }       else { true };
    let do_deps       = if any_explicit_scope { opts.include_deps }       else { true };
    let do_dependents = if any_explicit_scope { opts.include_dependents } else { false };

    // Save the originally listed packages so we can optionally exclude them later.
    let original_plans: HashSet<PathBuf> = plans_to_build.clone();

    // Use a scoped block to ensure the database handle (and its flock) is released
    // before we start the parallel build/install process.
    {
        let db_path = config.general.db_path.clone();
        let db = Database::open(&db_path).context("failed to open database for dependency resolution")?;

        // 1b. Upward expansion: resolve missing upstream deps.
        if opts.is_build_op() && do_deps {
            expand_missing_dependencies(&mut plans_to_build, &all_plans, &db, opts.rebuild_dependencies, actual_max)?;
        }
    }

    // 1c. Downward expansion: cascade link rebuilds to packages that depend on the targets.
    let reasons = if opts.is_build_op() && do_dependents {
        expand_rebuild_deps(&mut plans_to_build, &all_plans, opts.rebuild_dependents, actual_max)?
    } else {
        plans_to_build.iter()
            .filter_map(|p| PackageManifest::from_file(p).ok())
            .map(|m| (m.plan.name, RebuildReason::Explicit))
            .collect()
    };

    // 1d. If --self was not requested, remove the originally-listed packages from the
    //     build set. Their metadata was still used above to find deps/dependents.
    if opts.is_build_op() && !do_self {
        plans_to_build.retain(|p| !original_plans.contains(p));
        if plans_to_build.is_empty() {
            info!("Nothing to build: all requested deps/dependents are already satisfied.");
            return Ok(());
        }
    }

    // 2. Build dependency map
    let mut graph = build_dep_map(&plans_to_build, opts.checksum, reasons, &all_plans)?;

    // 2b. Detect and resolve bootstrap cycles
    if opts.is_build_op() {
        inject_bootstrap_passes(&mut graph)?;
    }

    // --- Build Plan Summary ---
    if !opts.quiet {
        eprintln!("Construction Plan:");
        let mut sorted_targets: Vec<_> = graph.build_set.iter().collect();
        sorted_targets.sort();
        for name in sorted_targets {
            let is_bootstrap_task = name.ends_with(":bootstrap");
            let base_name = name.trim_end_matches(":bootstrap");
            let is_full_after_bootstrap = !is_bootstrap_task
                && graph.build_set.contains(&format!("{}:bootstrap", name));

            let reason_str = if is_bootstrap_task {
                "[MVP]".to_string()
            } else if is_full_after_bootstrap {
                "[FULL]".to_string()
            } else {
                match graph.rebuild_reasons.get(name.as_str()) {
                    Some(RebuildReason::Explicit) => "[NEW]".to_string(),
                    Some(RebuildReason::LinkDependency) => "[LINK-REBUILD]".to_string(),
                    Some(RebuildReason::Transitive) => "[REV-REBUILD]".to_string(),
                    None => "".to_string(),
                }
            };
            eprintln!("  {: <15} {}", reason_str, base_name);
        }
        eprintln!();
    }

    // 3. Execute builds
    execute_builds(
        config,
        &graph.name_to_path,
        &graph.deps_map,
        &graph.build_set,
        &opts,
        &graph.bootstrap_excluded,
    )
}

// ---------------------------------------------------------------------------
// Resolver setup
// ---------------------------------------------------------------------------

pub fn setup_resolver(config: &GlobalConfig) -> Result<SimpleResolver> {
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
                    return Err(WrightError::BuildError(format!("Target not found: {}", clean_target)));
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
    pkg_to_plan: HashMap<String, String>,
    /// For bootstrap tasks (key = "{pkg}:bootstrap"), the deps that were
    /// excluded so the cycle could be broken.
    bootstrap_excluded: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
struct CycleCandidate {
    pkg: String,
    excluded: Vec<String>,
}

fn collect_phase_deps(
    manifest: &PackageManifest,
    pkg_to_plan: &HashMap<String, String>,
    is_mvp: bool,
) -> Vec<String> {
    let base = &manifest.dependencies;
    let overrides: Option<&PhaseDependencies> = if is_mvp {
        manifest.mvp.as_ref().and_then(|p| p.dependencies.as_ref())
    } else {
        None
    };

    let build = overrides
        .and_then(|o| o.build.clone())
        .unwrap_or_else(|| base.build.clone());
    let runtime = overrides
        .and_then(|o| o.runtime.clone())
        .unwrap_or_else(|| base.runtime.clone());
    let link = overrides
        .and_then(|o| o.link.clone())
        .unwrap_or_else(|| base.link.clone());

    let mut deps = Vec::new();
    let mut raw_deps = Vec::new();
    raw_deps.extend(build);
    raw_deps.extend(runtime);
    raw_deps.extend(link);

    for dep in raw_deps {
        let dep_pkg_name = version::parse_dependency(&dep)
            .unwrap_or_else(|_| (dep.clone(), None)).0;

        if let Some(parent_plan) = pkg_to_plan.get(&dep_pkg_name) {
            if parent_plan != &manifest.plan.name {
                deps.push(parent_plan.clone());
            }
        } else {
            deps.push(dep_pkg_name);
        }
    }

    deps
}

fn cycle_candidates_for(
    cycle: &[String],
    graph: &PlanGraph,
) -> Vec<CycleCandidate> {
    let cycle_set: HashSet<&str> = cycle.iter().map(|s| s.as_str()).collect();
    let mut candidates = Vec::new();

    for pkg in cycle {
        let path = match graph.name_to_path.get(pkg) {
            Some(p) => p,
            None => continue,
        };
        let manifest = match PackageManifest::from_file(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let has_mvp = manifest
            .mvp
            .as_ref()
            .and_then(|p| p.dependencies.as_ref())
            .is_some();
        if !has_mvp {
            continue;
        }

        let full_deps = collect_phase_deps(&manifest, &graph.pkg_to_plan, false);
        let mvp_deps = collect_phase_deps(&manifest, &graph.pkg_to_plan, true);

        let cycle_edges: Vec<String> = full_deps
            .iter()
            .filter(|d| cycle_set.contains(d.as_str()))
            .cloned()
            .collect();

        let excluded: Vec<String> = cycle_edges
            .iter()
            .filter(|d| !mvp_deps.contains(d))
            .cloned()
            .collect();

        if !excluded.is_empty() {
            candidates.push(CycleCandidate {
                pkg: pkg.clone(),
                excluded,
            });
        }
    }

    candidates
}

fn pick_candidate(mut candidates: Vec<CycleCandidate>) -> Option<CycleCandidate> {
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        let len_cmp = a.excluded.len().cmp(&b.excluded.len());
        if len_cmp == std::cmp::Ordering::Equal {
            a.pkg.cmp(&b.pkg)
        } else {
            len_cmp
        }
    });
    Some(candidates.remove(0))
}

// ---------------------------------------------------------------------------
// Bootstrap cycle detection (Tarjan's SCC)
// ---------------------------------------------------------------------------

struct SccState {
    index: usize,
    stack: Vec<String>,
    on_stack: HashMap<String, bool>,
    indices: HashMap<String, usize>,
    lowlinks: HashMap<String, usize>,
    sccs: Vec<Vec<String>>,
}

/// Return all strongly-connected components with more than one node.
fn find_cycles(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut state = SccState {
        index: 0,
        stack: Vec::new(),
        on_stack: HashMap::new(),
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        sccs: Vec::new(),
    };
    for node in graph.keys() {
        if !state.indices.contains_key(node.as_str()) {
            tarjan_visit(node, graph, &mut state);
        }
    }
    state.sccs
}

fn tarjan_visit(v: &str, graph: &HashMap<String, Vec<String>>, s: &mut SccState) {
    let idx = s.index;
    s.indices.insert(v.to_string(), idx);
    s.lowlinks.insert(v.to_string(), idx);
    s.index += 1;
    s.stack.push(v.to_string());
    s.on_stack.insert(v.to_string(), true);

    let neighbors = graph.get(v).cloned().unwrap_or_default();
    for w in &neighbors {
        if !s.indices.contains_key(w.as_str()) {
            tarjan_visit(w, graph, s);
            let ll_w = s.lowlinks[w.as_str()];
            *s.lowlinks.get_mut(v).expect("v was inserted at function entry") = s.lowlinks[v].min(ll_w);
        } else if *s.on_stack.get(w.as_str()).unwrap_or(&false) {
            let idx_w = s.indices[w.as_str()];
            *s.lowlinks.get_mut(v).expect("v was inserted at function entry") = s.lowlinks[v].min(idx_w);
        }
    }

    if s.lowlinks[v] == s.indices[v] {
        let mut scc = Vec::new();
        loop {
            let w = s.stack.pop().expect("stack must contain v and its descendants");
            s.on_stack.insert(w.clone(), false);
            scc.push(w.clone());
            if w == v { break; }
        }
        if scc.len() > 1 {
            s.sccs.push(scc);
        }
    }
}

/// For each dependency cycle in the graph, find a package with
/// `[phase.mvp].without` that breaks the cycle and insert a two-pass
/// build plan: `{pkg}:bootstrap` runs first (no cyclic dep), then
/// the rest of the cycle, then `{pkg}` (full) rebuilds with all deps.
fn inject_bootstrap_passes(graph: &mut PlanGraph) -> Result<()> {
    let cycles = find_cycles(&graph.deps_map);
    if cycles.is_empty() {
        info!("Dependency graph is acyclic.");
        return Ok(());
    }

    for cycle in &cycles {
        info!("Dependency cycle detected: {}", cycle.join(" → "));

        let candidates = cycle_candidates_for(cycle, graph);
        let chosen = pick_candidate(candidates.clone());

        let (pkg, excl) = match chosen {
            Some(c) => (c.pkg, c.excluded),
            None => {
                return Err(WrightError::BuildError(format!(
                    "Dependency cycle cannot be automatically resolved.\n\
                     Cycle: {}\n\
                     Add '[mvp.dependencies]' in one of these plans to declare \
                     an acyclic MVP dependency set.",
                    cycle.join(" → ")
                )));
            }
        };

        let bootstrap_key = format!("{}:bootstrap", pkg);

        // Bootstrap task: same deps as full, minus the cycle-breaking edges.
        let mvp_manifest = PackageManifest::from_file(&graph.name_to_path[&pkg])?;
        let bootstrap_deps = collect_phase_deps(&mvp_manifest, &graph.pkg_to_plan, true);

        graph.deps_map.insert(bootstrap_key.clone(), bootstrap_deps);
        graph.build_set.insert(bootstrap_key.clone());
        graph.name_to_path.insert(
            bootstrap_key.clone(),
            graph.name_to_path[&pkg].clone(),
        );
        graph.bootstrap_excluded.insert(bootstrap_key.clone(), excl.clone());

        // Full task now waits for its own bootstrap to finish.
        if let Some(deps) = graph.deps_map.get_mut(&pkg) {
            deps.push(bootstrap_key.clone());
        }

        // Other packages inside the cycle that depend on `pkg` should now
        // depend on the bootstrap version so they can start earlier.
        for other in cycle {
            if other == &pkg { continue; }
            if let Some(deps) = graph.deps_map.get_mut(other) {
                for dep in deps.iter_mut() {
                    if dep == &pkg {
                        *dep = bootstrap_key.clone();
                    }
                }
            }
        }

        info!(
            "Cycle resolved: '{}' will be built twice \
             (first as MVP without [{}], then fully)",
            pkg,
            excl.join(", ")
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Missing dependency expansion (Upward)
// ---------------------------------------------------------------------------

const SYSTEM_TOOLCHAIN: &[&str] = &["gcc", "glibc", "binutils", "make", "bison", "flex", "perl", "python", "texinfo", "m4", "sed", "gawk"];

fn expand_missing_dependencies(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
    force_all: bool,
    max_depth: usize,
) -> Result<()> {
    let mut build_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PackageManifest::from_file(path) {
            build_set.insert(m.plan.name.clone());
        }
    }

    let mut current_depth = 0;
    loop {
        if current_depth >= max_depth { break; }
        let mut added_any = false;
        let mut to_add_paths = Vec::new();

        for path in plans_to_build.iter() {
            let manifest = PackageManifest::from_file(path)?;
            
            // In a deep rebuild (-D), we want to cover build, link, and runtime dependencies.
            // For auto-resolving missing deps, we prioritize build and link as they are required for compilation.
            let deps_to_check = if force_all {
                manifest.dependencies.build.iter()
                    .chain(manifest.dependencies.link.iter())
                    .chain(manifest.dependencies.runtime.iter())
                    .collect::<Vec<_>>()
            } else {
                manifest.dependencies.build.iter()
                    .chain(manifest.dependencies.link.iter())
                    .collect::<Vec<_>>()
            };

            for dep in deps_to_check {
                let dep_name = version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None)).0;

                // Protect toolchain: don't automatically rebuild core tools unless they are missing
                if force_all && SYSTEM_TOOLCHAIN.contains(&dep_name.as_str()) {
                    continue;
                }

                if !build_set.contains(&dep_name) {
                    if force_all || db.get_package(&dep_name)?.is_none() {
                        if let Some(plan_path) = all_plans.get(&dep_name) {
                            info!("{} dependency (depth {}): {}", 
                                if force_all { "Forcing rebuild of" } else { "Auto-resolving missing" }, 
                                current_depth + 1, dep_name);
                            to_add_paths.push(plan_path.clone());
                            build_set.insert(dep_name.clone());
                            added_any = true;
                        }
                    }
                }
            }
        }

        for p in to_add_paths { plans_to_build.insert(p); }
        if !added_any { break; }
        current_depth += 1;
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
    max_depth: usize,
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
    let mut current_depth = 0;
    loop {
        if current_depth >= max_depth { break; }
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
                // PROTECTION: Do not automatically add system toolchain parts to the rebuild set 
                // via transitive link expansion unless rebuild_all (-R) is explicitly set.
                // This prevents "compiler-waiting-for-libc" deadlocks.
                if !rebuild_all && SYSTEM_TOOLCHAIN.contains(&name.as_str()) {
                    continue;
                }

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
        current_depth += 1;
    }

    Ok(reasons)
}

// ... (build_dep_map will need to take reasons)


fn build_dep_map(
    plans_to_build: &HashSet<PathBuf>,
    checksum: bool,
    rebuild_reasons: HashMap<String, RebuildReason>,
    all_plans: &HashMap<String, PathBuf>,
) -> Result<PlanGraph> {
    let mut name_to_path = HashMap::new();
    let mut deps_map = HashMap::new();
    let mut build_set = HashSet::new();

    // 1. Create a mapping of EVERY package name (main and splits) to its providing plan name.
    let mut pkg_to_plan = HashMap::new();
    for (plan_name, path) in all_plans {
        pkg_to_plan.insert(plan_name.clone(), plan_name.clone());
        if let Ok(m) = PackageManifest::from_file(path) {
            for split_name in m.split.keys() {
                pkg_to_plan.insert(split_name.clone(), plan_name.clone());
            }
        }
    }

    for path in plans_to_build {
        let manifest = PackageManifest::from_file(path)?;
        let name = manifest.plan.name.clone();
        name_to_path.insert(name.clone(), path.clone());
        build_set.insert(name.clone());

        let mut deps = Vec::new();
        if !checksum {
            deps = collect_phase_deps(&manifest, &pkg_to_plan, false);
        }
        deps_map.insert(name, deps);
    }

    Ok(PlanGraph {
        name_to_path,
        deps_map,
        build_set,
        rebuild_reasons,
        pkg_to_plan,
        bootstrap_excluded: HashMap::new(),
    })
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
    bootstrap_excluded: &HashMap<String, Vec<String>>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel::<std::result::Result<String, (String, WrightError)>>();
    let completed = Arc::new(Mutex::new(HashSet::<String>::new()));
    let in_progress = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_count = Arc::new(Mutex::new(0usize));

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());
    let install_lock = Arc::new(Mutex::new(())); // Serializes installation
    let bootstrap_excluded = Arc::new(bootstrap_excluded.clone());

    let actual_jobs = if opts.jobs == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
    } else {
        opts.jobs
    };

    info!("Starting build with {} parallel jobs", actual_jobs);

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let all_deps_met = opts.checksum || deps_map.get(name).unwrap().iter()
                        .filter(|d| build_set.contains(*d))
                        .all(|d| comp.contains(d));

                    if all_deps_met {
                        ready_to_launch.push(name.clone());
                    }
                }
            }
        }

        for name in ready_to_launch {
            if in_progress.lock().unwrap().len() >= actual_jobs {
                break;
            }

            in_progress.lock().unwrap().insert(name.clone());

            let tx_clone = tx.clone();
            let name_clone = name.clone();
            let path = name_to_path.get(&name).unwrap().clone();
            let builder_clone = builder.clone();
            let config_clone = config_arc.clone();
            let install_lock_clone = install_lock.clone();
            let bootstrap_excluded_clone = bootstrap_excluded.clone();

            // Bootstrap tasks: build without cyclic deps, set env vars.
            // Full tasks that follow a bootstrap: force rebuild (archive exists
            // but is the incomplete bootstrap version).
            let bootstrap_excl = bootstrap_excluded_clone
                .get(&name)
                .cloned()
                .unwrap_or_default();
            let is_post_bootstrap = !name.ends_with(":bootstrap")
                && build_set.contains(&format!("{}:bootstrap", name));
            let mut effective_opts = opts.clone();
            if is_post_bootstrap {
                effective_opts.force = true;
            }
            // Suppress subprocess output when running multiple jobs in parallel
            // to avoid interleaved output on the terminal.
            if actual_jobs > 1 {
                effective_opts.verbose = false;
            }

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
                    &effective_opts, &bootstrap_excl,
                );

                match res {
                    Ok(_) => {
                        // Success! Now install if requested
                        if effective_opts.install {
                            let _guard = install_lock_clone.lock().unwrap();
                            info!("Automatically installing built package: {}", name_clone);
                            
                            let output_dir = config_clone.general.components_dir.clone();
                            let archive_path = output_dir.join(manifest.archive_filename());
                            
                            match Database::open(&config_clone.general.db_path) {
                                Ok(db) => {
                                    // 1. Install main package
                                    if let Err(e) = crate::transaction::install_package(
                                        &db, &archive_path, &PathBuf::from("/"), true
                                    ) {
                                        error!("Build succeeded but automatic installation failed for {}: {:#}", name_clone, e);
                                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                                        return;
                                    }

                                    // 2. Install all split packages
                                    for split_name in manifest.split.keys() {
                                        let split_manifest = manifest.split.get(split_name).unwrap().to_manifest(split_name, &manifest);
                                        let split_archive_path = output_dir.join(split_manifest.archive_filename());
                                        info!("Automatically installing split package: {}", split_name);
                                        if let Err(e) = crate::transaction::install_package(
                                            &db, &split_archive_path, &PathBuf::from("/"), true
                                        ) {
                                            warn!("Automatic installation of split package '{}' failed: {:#}", split_name, e);
                                        }
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
            let mut message = String::from("Deadlock detected or dependency missing from plan set:\n");
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let missing: Vec<_> = deps_map.get(name).unwrap().iter()
                        .filter(|d| build_set.contains(*d) && !comp.contains(*d))
                        .cloned()
                        .collect();
                    message.push_str(&format!("  - {} is waiting for: {}\n", name, missing.join(", ")));
                }
            }
            return Err(WrightError::BuildError(message));
        }

        match rx.recv() {
            Err(_) => {
                return Err(WrightError::BuildError(
                    "build worker thread disconnected unexpectedly".to_string()
                ));
            }
            Ok(Ok(name)) => {
                in_progress.lock().unwrap().remove(&name);
                completed.lock().unwrap().insert(name.clone());
                if !opts.quiet {
                    eprintln!("[done] {}", name);
                }
            }
            Ok(Err((name, _))) => {
                in_progress.lock().unwrap().remove(&name);
                failed_set.lock().unwrap().insert(name.clone());
                *failed_count.lock().unwrap() += 1;
                if !opts.checksum {
                    return Err(WrightError::BuildError(format!("Construction failed due to error in {}", name)));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().unwrap();
    let final_completed = completed.lock().unwrap().len();

    if final_failed > 0 {
        warn!("Construction finished with {} successes and {} failures.", final_completed, final_failed);
        if !opts.checksum {
            return Err(WrightError::BuildError("Some parts failed to manufacture.".to_string()));
        }
    } else {
        info!("All {} tasks completed successfully.", final_completed);
    }

    Ok(())
}

fn lint_dependency_graph(
    plans_to_build: &HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
) -> Result<()> {
    let graph = build_dep_map(plans_to_build, false, HashMap::new(), all_plans)?;
    let cycles = find_cycles(&graph.deps_map);

    println!("Dependency Analysis Report");
    println!("Status: {}", if cycles.is_empty() { "acyclic" } else { "cyclic" });

    if cycles.is_empty() {
        return Ok(());
    }

    println!();
    println!("Cycles ({}):", cycles.len());
    for (idx, cycle) in cycles.iter().enumerate() {
        println!("{}: {}", idx + 1, cycle.join(" → "));
    }

    println!();
    println!("MVP Candidates (deterministic pick = fewest excluded edges, then name):");
    println!("Cycle | Candidate | Excludes | Selected");
    println!("----- | --------- | -------- | --------");
    for (idx, cycle) in cycles.iter().enumerate() {
        let candidates = cycle_candidates_for(cycle, &graph);
        if candidates.is_empty() {
            println!("{} | - | - | no candidates", idx + 1);
            continue;
        }
        let chosen = pick_candidate(candidates.clone());
        for cand in candidates {
            let selected = match &chosen {
                Some(c) if c.pkg == cand.pkg && c.excluded == cand.excluded => "yes",
                _ => "no",
            };
            println!(
                "{} | {} | {} | {}",
                idx + 1,
                cand.pkg,
                cand.excluded.join(", "),
                selected
            );
        }
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
    bootstrap_excl: &[String],
) -> Result<()> {
    if opts.checksum {
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

    // Build extra env vars for bootstrap pass.
    let mut extra_env = std::collections::HashMap::new();
    if !bootstrap_excl.is_empty() {
        if manifest.mvp.is_none() {
            warn!(
                "Package '{}' declares no [mvp.dependencies]; \
                 cannot compute MVP deps for cycle breaking.",
                manifest.plan.name
            );
        }
        extra_env.insert("WRIGHT_BOOTSTRAP_BUILD".to_string(), "1".to_string());
        extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
        for dep in bootstrap_excl {
            let key = format!(
                "WRIGHT_BOOTSTRAP_WITHOUT_{}",
                dep.to_uppercase().replace('-', "_")
            );
            extra_env.insert(key, "1".to_string());
        }
        info!(
            "MVP build of '{}' (without: {})",
            manifest.plan.name,
            bootstrap_excl.join(", ")
        );
    }

    if !extra_env.contains_key("WRIGHT_BUILD_PHASE") {
        extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "full".to_string());
    }
    info!("Manufacturing part {}...", manifest.plan.name);
    let plan_dir = manifest_path.parent().unwrap().to_path_buf();
    let result = builder.build(manifest, &plan_dir, opts.stage.clone(), opts.only.clone(), &extra_env, opts.verbose, opts.force)?;

    // Skip archive creation when --only is used for non-package stages
    if opts.only.is_none() || opts.only.as_deref() == Some("package") || opts.only.as_deref() == Some("post_package") {
        let archive_path = archive::create_archive(&result.pkg_dir, manifest, &output_dir)?;
        info!("Part stored in the Components Hold: {}", archive_path.display());

        for (split_name, split_pkg) in &manifest.split {
            let split_pkg_dir = result.split_pkg_dirs.get(split_name)
                .ok_or_else(|| WrightError::BuildError(format!("missing split pkg_dir for '{}'", split_name)))?;
            let split_manifest = split_pkg.to_manifest(split_name, manifest);
            let split_archive = archive::create_archive(split_pkg_dir, &split_manifest, &output_dir)?;
            info!("Split part stored: {}", split_archive.display());
        }
    }

    Ok(())
}
