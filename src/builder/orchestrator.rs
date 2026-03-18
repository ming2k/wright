//! Build orchestrator — parallel build scheduling, dependency resolution,
//! cascade expansion, and automatic bootstrap cycle detection/resolution.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::builder::mvp::{
    collect_phase_deps, cycle_candidates_for, find_cycles, format_cycle_path,
    inject_bootstrap_passes, pick_candidate, PlanGraph,
};
use crate::error::{Result, WrightError, WrightResultExt};
use tracing::{debug, error, info, warn};

use crate::builder::Builder;
use crate::config::{AssembliesConfig, GlobalConfig};
use crate::database::Database;
use crate::part::archive;
use crate::part::fhs;
use crate::part::version;
use crate::plan::manifest::{FabricateConfig, PlanManifest};
use crate::repo::source::SimpleResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DependencyMode {
    #[default]
    None,
    Missing,
    Sync,
    All,
}

/// Options for a build run.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Run only the specified lifecycle stages (in pipeline order); empty = run all.
    pub stages: Vec<String>,
    /// Internal: used by the Fetch command to run fetch/verify/extract only.
    pub fetch_only: bool,
    pub clean: bool,
    pub lint: bool,
    pub force: bool,
    pub checksum: bool,
    /// Skip the lifecycle `check` stage during a full build.
    /// Ignored for metadata-only operations and when explicit `--stage` selection is used.
    pub skip_check: bool,
    /// Max number of concurrently active dockyards.
    /// Only parts with no dependency relationship (direct or indirect)
    /// are scheduled simultaneously. 0 = auto-detect CPU count.
    pub dockyards: usize,
    pub rebuild_dependents: bool,
    pub deps_mode: DependencyMode,
    pub install: bool,
    pub depth: Option<usize>,
    pub verbose: bool,
    pub quiet: bool,
    /// --self (-s): include the listed parts themselves in the build.
    pub include_self: bool,
    /// --dependents: include downstream link-rebuild dependents (not the listed parts themselves).
    pub include_dependents: bool,
    /// --mvp: build using [mvp.dependencies] without requiring a cycle to trigger it.
    pub mvp: bool,
    /// Per-dockyard NPROC hint: how many compiler threads each dockyard should use.
    /// The scheduler computes this per launched task from the currently active
    /// dockyard count (`total_cpus / active_dockyards`) so resource share adapts as
    /// dependency levels fan out or collapse. None means let the builder fall
    /// back to its own logic.
    pub nproc_per_dockyard: Option<u32>,
}

impl BuildOptions {
    /// Returns true if this is a real build operation.
    /// Per-plan metadata operations (checksum, lint, fetch) skip all
    /// dependency cascade expansion.
    fn is_build_op(&self) -> bool {
        !self.checksum && !self.lint && !self.fetch_only
    }
}

/// Run a multi-target build with dependency ordering and parallel execution.
pub fn run_build(config: &GlobalConfig, targets: Vec<String>, opts: BuildOptions) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let mut plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::BuildError(
            "No targets specified to build.".to_string(),
        ));
    }

    if opts.lint {
        return lint_dependency_graph(&plans_to_build, &all_plans);
    }

    let actual_max = {
        let max_depth = opts.depth.unwrap_or(1);
        if max_depth == 0 {
            usize::MAX
        } else {
            max_depth
        }
    };

    // Determine effective scope.
    // Default: build the listed targets only.
    // Dependency expansion is opt-in via --deps <mode>.
    // Reverse rebuilds remain opt-in via --dependents.
    let do_deps = opts.deps_mode != DependencyMode::None;
    let do_dependents = opts.include_dependents;
    let do_self = opts.include_self || !do_dependents || do_deps;

    // Save the originally listed parts so we can optionally exclude them later.
    let original_plans: HashSet<PathBuf> = plans_to_build.clone();

    // Use a scoped block to ensure the database handle (and its flock) is released
    // before we start the parallel build/install process.
    {
        let db_path = config.general.db_path.clone();
        let db = Database::open(&db_path)
            .context("failed to open database for dependency resolution")?;

        // 1a. Upward expansion: resolve upstream deps according to --deps mode.
        if opts.is_build_op() && do_deps {
            expand_missing_dependencies(
                &mut plans_to_build,
                &all_plans,
                &db,
                opts.deps_mode,
                opts.install,
                actual_max,
            )?;
        }
    }

    // 1c. Downward expansion: cascade link rebuilds to parts that depend on the targets.
    let reasons = if opts.is_build_op() && do_dependents {
        expand_rebuild_deps(
            &mut plans_to_build,
            &all_plans,
            opts.rebuild_dependents,
            actual_max,
        )?
    } else {
        plans_to_build
            .iter()
            .filter_map(|p| PlanManifest::from_file(p).ok())
            .map(|m| (m.plan.name, RebuildReason::Explicit))
            .collect()
    };

    // 1d. If --self was not requested, remove the originally-listed parts from the
    //     build set. Their metadata was still used above to find deps/dependents.
    if opts.is_build_op() && !do_self {
        plans_to_build.retain(|p| !original_plans.contains(p));
        if plans_to_build.is_empty() {
            info!("Nothing to build: all requested deps/dependents are already satisfied.");
            return Ok(());
        }
    }

    // 2. Build dependency map
    let mut graph = build_dep_map(
        &plans_to_build,
        opts.checksum,
        opts.mvp,
        reasons,
        &all_plans,
    )?;

    // 2b. Detect and resolve bootstrap cycles (skip when --mvp: already using MVP deps)
    if opts.is_build_op() && !opts.mvp {
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
            let is_full_after_bootstrap =
                !is_bootstrap_task && graph.build_set.contains(&format!("{}:bootstrap", name));

            let reason_str = if is_bootstrap_task || opts.mvp {
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

    // Derive the set of user-specified target names (for install_reason tracking).
    let user_target_names: HashSet<String> = original_plans
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| m.plan.name)
        .collect();

    // 3. Execute builds
    execute_builds(
        config,
        &graph.name_to_path,
        &graph.deps_map,
        &graph.build_set,
        &opts,
        &graph.bootstrap_excluded,
        &user_target_names,
    )
}

// ---------------------------------------------------------------------------
// Resolver setup
// ---------------------------------------------------------------------------

pub fn setup_resolver(config: &GlobalConfig) -> Result<SimpleResolver> {
    let mut all_assemblies = AssembliesConfig {
        assemblies: HashMap::new(),
    };

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
    resolver.set_repo_db_path(config.general.repo_db_path.clone());
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
        if clean_target.is_empty() {
            continue;
        }

        if let Some(assembly_name) = clean_target.strip_prefix('@') {
            let paths = resolver.resolve_assembly(assembly_name)?;
            if paths.is_empty() {
                warn!("Assembly not found: {}", assembly_name);
            }
            for p in paths {
                plans_to_build.insert(p);
            }
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
                        PlanManifest::from_file(&candidate)
                            .context(format!("failed to parse plan '{}'", clean_target))?;
                        plans_to_build.insert(candidate);
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(WrightError::BuildError(format!(
                        "Target not found: {}",
                        clean_target
                    )));
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

// ---------------------------------------------------------------------------
// Missing dependency expansion (Upward)
// ---------------------------------------------------------------------------

const SYSTEM_TOOLCHAIN: &[&str] = &[
    "gcc", "glibc", "binutils", "make", "bison", "flex", "perl", "python", "texinfo", "m4", "sed",
    "gawk",
];

fn expand_missing_dependencies(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
    mode: DependencyMode,
    include_runtime: bool,
    max_depth: usize,
) -> Result<()> {
    let mut build_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PlanManifest::from_file(path) {
            build_set.insert(m.plan.name.clone());
        }
    }

    let mut current_depth = 0;
    loop {
        if current_depth >= max_depth {
            break;
        }
        let mut added_any = false;
        let mut to_add_paths = Vec::new();

        for path in plans_to_build.iter() {
            let manifest = PlanManifest::from_file(path)?;

            // `all` covers build, link, and runtime recursively.
            // With --install (-i), we also resolve runtime deps because they are
            // required post-install.
            let deps_to_check = if matches!(mode, DependencyMode::All) || include_runtime {
                manifest
                    .dependencies
                    .build
                    .iter()
                    .chain(manifest.dependencies.link.iter())
                    .chain(manifest.dependencies.runtime.iter())
                    .collect::<Vec<_>>()
            } else {
                manifest
                    .dependencies
                    .build
                    .iter()
                    .chain(manifest.dependencies.link.iter())
                    .collect::<Vec<_>>()
            };

            for dep in deps_to_check {
                let dep_name = version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None))
                    .0;

                // Protect toolchain: don't automatically rebuild core tools unless they are missing
                if matches!(mode, DependencyMode::All) && SYSTEM_TOOLCHAIN.contains(&dep_name.as_str()) {
                    continue;
                }

                if !build_set.contains(&dep_name) {
                    let should_add = dependency_requires_build(&dep_name, all_plans, db, mode)?;
                    if should_add {
                        if let Some(plan_path) = all_plans.get(&dep_name) {
                            info!(
                                "{} dependency (depth {}): {}",
                                dependency_action_label(&dep_name, all_plans, db, mode)?,
                                current_depth + 1,
                                dep_name
                            );
                            to_add_paths.push(plan_path.clone());
                            build_set.insert(dep_name.clone());
                            added_any = true;
                        }
                    }
                }
            }
        }

        // Ensure the full transitive runtime dependency closure of every build
        // dependency is present. A build dep like python-sphinx is useless if
        // any part in its runtime dep tree (python-requests → python-urllib3 …)
        // is missing.  We use BFS to expand the complete closure.
        if !matches!(mode, DependencyMode::All) {
            let snapshot: Vec<PathBuf> = plans_to_build
                .iter()
                .chain(to_add_paths.iter())
                .cloned()
                .collect();
            for path in &snapshot {
                let manifest = match PlanManifest::from_file(path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                for build_dep in &manifest.dependencies.build {
                    let build_dep_name = version::parse_dependency(build_dep)
                        .unwrap_or_else(|_| (build_dep.clone(), None))
                        .0;

                    let build_dep_depth = current_depth + 1;
                    if build_dep_depth >= max_depth {
                        continue;
                    }

                    // BFS over the runtime dep tree of this build dependency.
                    // Track absolute graph depth from the original target so
                    // logs and --depth limiting use real edge distance.
                    let mut queue = std::collections::VecDeque::new();
                    queue.push_back((build_dep_name.clone(), build_dep_depth));
                    let mut visited = HashSet::new();
                    visited.insert(build_dep_name.clone());

                    while let Some((cur, cur_depth)) = queue.pop_front() {
                        if let Some(cur_plan_path) = all_plans.get(&cur) {
                            if let Ok(cur_manifest) = PlanManifest::from_file(cur_plan_path) {
                                for rdep in &cur_manifest.dependencies.runtime {
                                    let rdep_name = version::parse_dependency(rdep)
                                        .unwrap_or_else(|_| (rdep.clone(), None))
                                        .0;
                                    if !visited.insert(rdep_name.clone()) {
                                        continue;
                                    }
                                    let rdep_depth = cur_depth + 1;
                                    if rdep_depth > max_depth {
                                        continue;
                                    }
                                    if !build_set.contains(&rdep_name)
                                        && dependency_requires_build(
                                            &rdep_name,
                                            all_plans,
                                            db,
                                            mode,
                                        )?
                                    {
                                        if let Some(rdep_plan_path) = all_plans.get(&rdep_name) {
                                            info!(
                                                "{} transitive runtime dep of build dep {} (depth {}): {}",
                                                dependency_action_label(&rdep_name, all_plans, db, mode)?,
                                                build_dep_name,
                                                rdep_depth,
                                                rdep_name
                                            );
                                            to_add_paths.push(rdep_plan_path.clone());
                                            build_set.insert(rdep_name.clone());
                                            added_any = true;
                                        }
                                    }
                                    // Continue BFS regardless of whether rdep was missing,
                                    // since its own runtime deps may still be missing.
                                    queue.push_back((rdep_name, rdep_depth));
                                }
                            }
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
        current_depth += 1;
    }

    Ok(())
}

fn dependency_action_label(
    dep_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
    mode: DependencyMode,
) -> Result<&'static str> {
    match mode {
        DependencyMode::All => Ok("Forcing rebuild of"),
        DependencyMode::Missing => Ok("Auto-resolving missing"),
        DependencyMode::Sync => {
            if dependency_plan_differs(dep_name, all_plans, db)? {
                Ok("Syncing outdated")
            } else {
                Ok("Auto-resolving missing")
            }
        }
        DependencyMode::None => Ok("Skipping"),
    }
}

fn dependency_requires_build(
    dep_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
    mode: DependencyMode,
) -> Result<bool> {
    match mode {
        DependencyMode::None => Ok(false),
        DependencyMode::All => Ok(true),
        DependencyMode::Missing => Ok(db.get_part(dep_name)?.is_none()),
        DependencyMode::Sync => {
            if db.get_part(dep_name)?.is_none() {
                return Ok(true);
            }
            dependency_plan_differs(dep_name, all_plans, db)
        }
    }
}

fn dependency_plan_differs(
    dep_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
) -> Result<bool> {
    let Some(installed) = db.get_part(dep_name)? else {
        return Ok(true);
    };
    let Some(plan_path) = all_plans.get(dep_name) else {
        return Ok(false);
    };
    let manifest = PlanManifest::from_file(plan_path)?;
    Ok(!installed_matches_manifest(&installed, &manifest))
}

fn installed_matches_manifest(
    installed: &crate::database::InstalledPart,
    manifest: &PlanManifest,
) -> bool {
    installed.epoch == manifest.plan.epoch
        && installed.version == manifest.plan.version
        && installed.release == manifest.plan.release
}

#[cfg(test)]
mod tests {
    use super::installed_matches_manifest;
    use crate::database::InstalledPart;
    use crate::plan::manifest::PlanManifest;

    fn installed_part(version: &str, release: u32, epoch: u32) -> InstalledPart {
        InstalledPart {
            id: 1,
            name: "zlib".to_string(),
            version: version.to_string(),
            release,
            epoch,
            description: "test".to_string(),
            arch: "x86_64".to_string(),
            license: "Zlib".to_string(),
            url: None,
            installed_at: "now".to_string(),
            install_size: 1,
            pkg_hash: None,
            install_scripts: None,
            assumed: false,
            install_reason: "explicit".to_string(),
        }
    }

    #[test]
    fn installed_matches_manifest_requires_epoch_version_and_release_match() {
        let manifest = PlanManifest::parse(
            r#"
name = "zlib"
version = "1.3.1"
release = 2
epoch = 1
description = "test part"
license = "Zlib"
arch = "x86_64"

[lifecycle.staging]
executor = "shell"
dockyard = "none"
script = "mkdir -p ${PART_DIR}/usr/lib"
"#,
        )
        .unwrap();

        assert!(installed_matches_manifest(
            &installed_part("1.3.1", 2, 1),
            &manifest
        ));
        assert!(!installed_matches_manifest(
            &installed_part("1.3.0", 2, 1),
            &manifest
        ));
        assert!(!installed_matches_manifest(
            &installed_part("1.3.1", 1, 1),
            &manifest
        ));
        assert!(!installed_matches_manifest(
            &installed_part("1.3.1", 2, 0),
            &manifest
        ));
    }
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
        if let Ok(m) = PlanManifest::from_file(plan_path) {
            let br_deps: Vec<String> = m
                .dependencies
                .runtime
                .iter()
                .chain(m.dependencies.build.iter())
                .map(|d| {
                    version::parse_dependency(d)
                        .unwrap_or_else(|_| (d.clone(), None))
                        .0
                })
                .collect();
            let l_deps: Vec<String> = m
                .dependencies
                .link
                .iter()
                .map(|d| {
                    version::parse_dependency(d)
                        .unwrap_or_else(|_| (d.clone(), None))
                        .0
                })
                .collect();

            build_runtime_deps.insert(plan_name.clone(), br_deps);
            link_deps.insert(plan_name.clone(), l_deps);
            all_name_to_path.insert(plan_name.clone(), plan_path.clone());
        }
    }

    // 2. Initial rebuild set
    let mut rebuild_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PlanManifest::from_file(path) {
            let name = m.plan.name.clone();
            rebuild_set.insert(name.clone());
            reasons.insert(name, RebuildReason::Explicit);
        }
    }

    // 3. Transitively expand
    let mut current_depth = 0;
    loop {
        if current_depth >= max_depth {
            break;
        }
        let mut added = false;
        for (name, path) in &all_name_to_path {
            if rebuild_set.contains(name) {
                continue;
            }

            let link_changed = link_deps
                .get(name)
                .map_or(false, |deps| deps.iter().any(|d| rebuild_set.contains(d)));

            let other_changed = rebuild_all
                && build_runtime_deps
                    .get(name)
                    .map_or(false, |deps| deps.iter().any(|d| rebuild_set.contains(d)));

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
                    if link_changed {
                        RebuildReason::LinkDependency
                    } else {
                        RebuildReason::Transitive
                    },
                );
                added = true;
            }
        }
        if !added {
            break;
        }
        current_depth += 1;
    }

    Ok(reasons)
}

// ... (build_dep_map will need to take reasons)

fn build_dep_map(
    plans_to_build: &HashSet<PathBuf>,
    checksum: bool,
    is_mvp: bool,
    rebuild_reasons: HashMap<String, RebuildReason>,
    all_plans: &HashMap<String, PathBuf>,
) -> Result<PlanGraph> {
    let mut name_to_path = HashMap::new();
    let mut deps_map = HashMap::new();
    let mut build_set = HashSet::new();
    let mut bootstrap_excluded = HashMap::new();

    // 1. Create a mapping of every part name (main and splits) to its providing plan name.
    let mut pkg_to_plan = HashMap::new();
    for (plan_name, path) in all_plans {
        pkg_to_plan.insert(plan_name.clone(), plan_name.clone());
        if let Ok(m) = PlanManifest::from_file(path) {
            if let Some(FabricateConfig::Multi(ref pkgs)) = m.fabricate {
                for sub_name in pkgs.keys() {
                    if sub_name != &m.plan.name {
                        pkg_to_plan.insert(sub_name.clone(), plan_name.clone());
                    }
                }
            }
        }
    }

    for path in plans_to_build {
        let manifest = PlanManifest::from_file(path)?;
        let name = manifest.plan.name.clone();
        name_to_path.insert(name.clone(), path.clone());
        build_set.insert(name.clone());

        let mut deps = Vec::new();
        if !checksum {
            deps = collect_phase_deps(&manifest, &pkg_to_plan, is_mvp, Some(all_plans));

            // For explicit --mvp builds, compute which deps are excluded vs. full
            // so build_one can pass the right WRIGHT_BOOTSTRAP_WITHOUT_* env vars.
            if is_mvp {
                let full_deps = collect_phase_deps(&manifest, &pkg_to_plan, false, Some(all_plans));
                let mvp_deps = collect_phase_deps(&manifest, &pkg_to_plan, true, Some(all_plans));
                let excluded: Vec<String> = full_deps
                    .into_iter()
                    .filter(|d| !mvp_deps.contains(d))
                    .collect();
                if !excluded.is_empty() {
                    bootstrap_excluded.insert(name.clone(), excluded);
                }
            }
        }
        deps_map.insert(name, deps);
    }

    Ok(PlanGraph {
        name_to_path,
        deps_map,
        build_set,
        rebuild_reasons,
        pkg_to_plan,
        bootstrap_excluded,
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
    user_target_names: &HashSet<String>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel::<std::result::Result<String, (String, WrightError)>>();
    let completed = Arc::new(Mutex::new(HashSet::<String>::new()));
    let in_progress = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_count = Arc::new(Mutex::new(0usize));

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());
    let install_lock = Arc::new(Mutex::new(())); // Serializes installation
    let compile_lock = Arc::new(Mutex::new(())); // Serializes compile stages
    let bootstrap_excluded = Arc::new(bootstrap_excluded.clone());

    let available_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let total_cpus = if let Some(cap) = config.build.max_cpus {
        available_cpus.min(cap.max(1))
    } else {
        available_cpus
    };
    let actual_dockyards = if opts.dockyards == 0 {
        total_cpus
    } else {
        opts.dockyards.min(total_cpus)
    };

    info!(
        "CPUs: {}  |  compile: one-at-a-time across dockyards",
        total_cpus
    );

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let all_deps_met = opts.checksum
                        || deps_map
                            .get(name)
                            .unwrap()
                            .iter()
                            .filter(|d| build_set.contains(*d))
                            .all(|d| comp.contains(d));

                    if all_deps_met {
                        ready_to_launch.push(name.clone());
                    }
                }
            }
        }

        // Launch a bounded batch and compute a fair CPU share for the entire
        // wave up front. This avoids sequential allocations like 16/1, 16/2,
        // 16/3 in the same wave, which makes the displayed shares additive
        // beyond the machine capacity.
        let base_active = in_progress.lock().unwrap().len();
        let free_slots = actual_dockyards.saturating_sub(base_active);
        let launch_batch: Vec<_> = ready_to_launch.into_iter().take(free_slots).collect();
        let planned_active = base_active + launch_batch.len();

        for (launch_idx, name) in launch_batch.into_iter().enumerate() {
            // Track active dockyards and derive a CPU budget for this launch.
            let active_dockyards = {
                let mut in_progress_guard = in_progress.lock().unwrap();
                in_progress_guard.insert(name.clone());
                in_progress_guard.len()
            };

            // Use static config override if provided; otherwise partition CPUs
            // across the active set size planned for this launch wave. Remainder
            // CPUs are handed to the earliest positions in that wave.
            let dynamic_nproc_cap = if let Some(n) = opts.nproc_per_dockyard {
                Some(n)
            } else {
                let base_share = (total_cpus / planned_active.max(1)).max(1);
                let remainder = total_cpus % planned_active.max(1);
                let active_position = base_active + launch_idx + 1;
                let share = if active_position <= remainder {
                    base_share + 1
                } else {
                    base_share
                };
                Some(share as u32)
            };

            info!("[dockyard {}] {}", active_dockyards, name);

            let tx_clone = tx.clone();
            let name_clone = name.clone();
            let path = name_to_path.get(&name).unwrap().clone();
            let builder_clone = builder.clone();
            let config_clone = config_arc.clone();
            let install_lock_clone = install_lock.clone();
            let compile_lock_clone = compile_lock.clone();
            let bootstrap_excluded_clone = bootstrap_excluded.clone();
            let is_user_target = user_target_names.contains(name.trim_end_matches(":bootstrap"));

            // Bootstrap tasks: build without cyclic deps, set env vars.
            // Full tasks that follow a bootstrap: force rebuild (archive exists
            // but is the incomplete bootstrap version).
            let bootstrap_excl = bootstrap_excluded_clone
                .get(&name)
                .cloned()
                .unwrap_or_default();
            let is_post_bootstrap =
                !name.ends_with(":bootstrap") && build_set.contains(&format!("{}:bootstrap", name));
            let mut effective_opts = opts.clone();
            if is_post_bootstrap {
                effective_opts.force = true;
            }
            // Output routing rules:
            //   single dockyard + no --quiet  → stream subprocess output to terminal (like makepkg/emerge)
            //   multi  dockyard + no --verbose → suppress to avoid interleaved terminal noise
            //   multi  dockyard + --verbose   → user explicitly asked; show (may interleave)
            if actual_dockyards == 1 && !opts.quiet {
                effective_opts.verbose = true;
            } else if actual_dockyards > 1 && !opts.verbose {
                effective_opts.verbose = false;
            }
            // else: multi-dockyard + explicit -v → keep opts.verbose = true (user's choice)
            effective_opts.nproc_per_dockyard = dynamic_nproc_cap;

            std::thread::spawn(move || {
                let manifest = match PlanManifest::from_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                        return;
                    }
                };
                let res = build_one(
                    &builder_clone,
                    &manifest,
                    &path,
                    &config_clone,
                    &effective_opts,
                    &bootstrap_excl,
                    compile_lock_clone.clone(),
                );

                match res {
                    Ok(_) => {
                        // Success! Now install if requested
                        if effective_opts.install {
                            let _guard = install_lock_clone.lock().unwrap();
                            debug!("Automatically installing built part: {}", name_clone);

                            let output_dir = config_clone.general.components_dir.clone();
                            let archive_path = output_dir.join(manifest.archive_filename());

                            match Database::open(&config_clone.general.db_path) {
                                Ok(db) => {
                                    // Determine install reason:
                                    // - Already installed → preserve existing reason (upgrade path handles this)
                                    // - New + user target → explicit
                                    // - New + auto-resolved dep → dependency
                                    let reason = if is_user_target {
                                        "explicit"
                                    } else {
                                        "dependency"
                                    };

                                    // 1. Install main part
                                    if let Err(e) = crate::transaction::install_part_with_reason(
                                        &db,
                                        &archive_path,
                                        &PathBuf::from("/"),
                                        true,
                                        reason,
                                    ) {
                                        error!("Build succeeded but automatic installation failed for {}: {:#}", name_clone, e);
                                        tx_clone.send(Err((name_clone, e.into()))).unwrap();
                                        return;
                                    }

                                    // 2. Install all sub-parts
                                    if let Some(FabricateConfig::Multi(ref pkgs)) =
                                        manifest.fabricate
                                    {
                                        for (sub_name, sub_pkg) in pkgs {
                                            if sub_name == &manifest.plan.name {
                                                continue;
                                            }
                                            let sub_manifest =
                                                sub_pkg.to_manifest(sub_name, &manifest);
                                            let sub_archive_path =
                                                output_dir.join(sub_manifest.archive_filename());
                                            debug!(
                                                "Automatically installing sub-part: {}",
                                                sub_name
                                            );
                                            if let Err(e) =
                                                crate::transaction::install_part_with_reason(
                                                    &db,
                                                    &sub_archive_path,
                                                    &PathBuf::from("/"),
                                                    true,
                                                    reason,
                                                )
                                            {
                                                warn!("Automatic installation of sub-part '{}' failed: {:#}", sub_name, e);
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to open database for automatic installation: {:#}",
                                        e
                                    );
                                    tx_clone.send(Err((name_clone, e.into()))).unwrap();
                                    return;
                                }
                            }
                        }
                        tx_clone.send(Ok(name_clone)).unwrap();
                    }
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
            let mut message =
                String::from("Deadlock detected or dependency missing from plan set:\n");
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let missing: Vec<_> = deps_map
                        .get(name)
                        .unwrap()
                        .iter()
                        .filter(|d| build_set.contains(*d) && !comp.contains(*d))
                        .cloned()
                        .collect();
                    message.push_str(&format!(
                        "  - {} is waiting for: {}\n",
                        name,
                        missing.join(", ")
                    ));
                }
            }
            return Err(WrightError::BuildError(message));
        }

        match rx.recv() {
            Err(_) => {
                return Err(WrightError::BuildError(
                    "dockyard thread disconnected unexpectedly".to_string(),
                ));
            }
            Ok(Ok(name)) => {
                in_progress.lock().unwrap().remove(&name);
                completed.lock().unwrap().insert(name.clone());
                if !opts.quiet {
                    info!("[done] {}", name);
                }
            }
            Ok(Err((name, _))) => {
                in_progress.lock().unwrap().remove(&name);
                failed_set.lock().unwrap().insert(name.clone());
                *failed_count.lock().unwrap() += 1;
                if !opts.checksum {
                    return Err(WrightError::BuildError(format!(
                        "Construction failed due to error in {}",
                        name
                    )));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().unwrap();
    let final_completed = completed.lock().unwrap().len();

    if final_failed > 0 {
        warn!(
            "Construction finished with {} successes and {} failures.",
            final_completed, final_failed
        );
        if !opts.checksum {
            return Err(WrightError::BuildError(
                "Some parts failed to manufacture.".to_string(),
            ));
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
    let graph = build_dep_map(plans_to_build, false, false, HashMap::new(), all_plans)?;
    let cycles = find_cycles(&graph.deps_map);

    println!("Dependency Analysis Report");
    println!(
        "Status: {}",
        if cycles.is_empty() {
            "acyclic"
        } else {
            "cyclic"
        }
    );

    if cycles.is_empty() {
        return Ok(());
    }

    println!();
    println!("Cycles ({}):", cycles.len());
    for (idx, cycle) in cycles.iter().enumerate() {
        println!("{}: {}", idx + 1, format_cycle_path(cycle, &graph.deps_map));
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
// Single part build
// ---------------------------------------------------------------------------

fn build_one(
    builder: &Builder,
    manifest: &PlanManifest,
    manifest_path: &Path,
    config: &GlobalConfig,
    opts: &BuildOptions,
    bootstrap_excl: &[String],
    compile_lock: Arc<Mutex<()>>,
) -> Result<()> {
    if opts.checksum {
        builder
            .update_hashes(manifest, manifest_path)
            .context("failed to update hashes")?;
        info!("Updated plan hashes: {}", manifest.plan.name);
        return Ok(());
    }

    if opts.lint {
        println!(
            "valid plan: {} {}-{}",
            manifest.plan.name, manifest.plan.version, manifest.plan.release
        );
        if let Some(FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
            for sub_name in pkgs.keys() {
                if sub_name != &manifest.plan.name {
                    println!("  sub-part: {}", sub_name);
                }
            }
        }
        return Ok(());
    }

    if opts.clean {
        builder
            .clean(manifest)
            .context("failed to clean workspace")?;
    }

    let output_dir = if config.general.components_dir.exists()
        || std::fs::create_dir_all(&config.general.components_dir).is_ok()
    {
        config.general.components_dir.clone()
    } else {
        std::env::current_dir()?
    };

    // Skip if archive already exists (unless --force or specific stages are requested)
    if !opts.force && opts.stages.is_empty() && !opts.fetch_only {
        let archive_name = manifest.archive_filename();
        let existing = output_dir.join(&archive_name);
        let all_exist = existing.exists()
            && match manifest.fabricate {
                Some(FabricateConfig::Multi(ref pkgs)) => pkgs
                    .iter()
                    .filter(|(name, _)| *name != &manifest.plan.name)
                    .all(|(sub_name, sub_pkg)| {
                        let sub_manifest = sub_pkg.to_manifest(sub_name, manifest);
                        output_dir.join(sub_manifest.archive_filename()).exists()
                    }),
                _ => true,
            };
        if all_exist && existing.exists() {
            info!(
                "Skipping {} (all archives already exist, use --force to rebuild)",
                manifest.plan.name
            );
            return Ok(());
        }
    }

    // Build extra env vars for bootstrap/MVP pass.
    let mut extra_env = std::collections::HashMap::new();
    if !bootstrap_excl.is_empty() || opts.mvp {
        if manifest.mvp.is_none() && !bootstrap_excl.is_empty() {
            warn!(
                "Plan '{}' declares no [mvp.dependencies]; \
                 cannot compute MVP deps for cycle breaking.",
                manifest.plan.name
            );
        }
        extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
        for dep in bootstrap_excl {
            let key = format!(
                "WRIGHT_BOOTSTRAP_WITHOUT_{}",
                dep.to_uppercase().replace('-', "_")
            );
            extra_env.insert(key, "1".to_string());
        }
        if !bootstrap_excl.is_empty() {
            info!(
                "MVP build of '{}' (without: {})",
                manifest.plan.name,
                bootstrap_excl.join(", ")
            );
        } else {
            info!("MVP build of '{}'", manifest.plan.name);
        }
    }

    if !extra_env.contains_key("WRIGHT_BUILD_PHASE") {
        extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "full".to_string());
    }
    info!("Manufacturing part {}...", manifest.plan.name);
    let plan_dir = manifest_path.parent().unwrap().to_path_buf();
    let result = builder.build(
        manifest,
        &plan_dir,
        &opts.stages,
        opts.fetch_only,
        opts.skip_check,
        &extra_env,
        opts.verbose,
        opts.force,
        opts.nproc_per_dockyard,
        Some(compile_lock),
    )?;

    // Full builds always end in archive creation. For explicit stage runs, only
    // produce output when the selection reaches the final fabricate phase.
    // Plans with no fabricate stage or fabricate output metadata still treat
    // staging as the final output-producing step for compatibility.
    let has_fabricate_stage = manifest.fabricate.is_some()
        || manifest.lifecycle.contains_key("fabricate")
        || manifest.lifecycle.contains_key("pre_fabricate")
        || manifest.lifecycle.contains_key("post_fabricate");
    let explicit_output_stage = opts
        .stages
        .iter()
        .any(|s| s == "fabricate" || s == "post_fabricate");
    let legacy_output_stage = !has_fabricate_stage
        && opts
            .stages
            .iter()
            .any(|s| s == "staging" || s == "post_staging");
    let produces_output = opts.stages.is_empty() || explicit_output_stage || legacy_output_stage;
    if produces_output && !opts.fetch_only {
        if !manifest.options.skip_fhs_check {
            fhs::validate(&result.pkg_dir, &manifest.plan.name)?;
        }
        let archive_path = archive::create_archive(&result.pkg_dir, manifest, &output_dir)?;
        info!(
            "{}: part stored in {}",
            manifest.plan.name,
            archive_path.display()
        );
        register_in_repo(&config.general.repo_db_path, &archive_path);

        if let Some(FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
            for (sub_name, sub_pkg) in pkgs {
                if sub_name == &manifest.plan.name {
                    continue;
                }
                let sub_pkg_dir = result.split_pkg_dirs.get(sub_name).ok_or_else(|| {
                    WrightError::BuildError(format!(
                        "missing sub-part pkg_dir for '{}'",
                        sub_name
                    ))
                })?;
                if !manifest.options.skip_fhs_check {
                    fhs::validate(sub_pkg_dir, sub_name)?;
                }
                let sub_manifest = sub_pkg.to_manifest(sub_name, manifest);
                let sub_archive = archive::create_archive(sub_pkg_dir, &sub_manifest, &output_dir)?;
                info!("{}: part stored in {}", sub_name, sub_archive.display());
                register_in_repo(&config.general.repo_db_path, &sub_archive);
            }
        }
    }

    Ok(())
}

/// Register a built archive in the repo database. Failures are logged but
/// do not abort the build — the archive is already on disk and can be
/// imported later via `wrepo sync`.
fn register_in_repo(repo_db_path: &Path, archive_path: &Path) {
    let do_register = || -> Result<()> {
        let repo_db = crate::repo::db::RepoDb::open(repo_db_path)?;
        let partinfo = archive::read_partinfo(archive_path)?;
        let sha256 = crate::util::checksum::sha256_file(archive_path)?;
        let filename = archive_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        repo_db.register_part(&partinfo, filename, &sha256)?;
        Ok(())
    };
    if let Err(e) = do_register() {
        warn!("Failed to register in repo DB: {}", e);
    }
}
