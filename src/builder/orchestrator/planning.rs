use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use tracing::info;

use crate::builder::mvp::{collect_phase_deps, PlanGraph};
use crate::database::{Database, InstalledPart};
use crate::error::Result;
use crate::part::version;
use crate::plan::manifest::{FabricateConfig, PlanManifest};

use super::{BuildOptions, DependencyMode, RebuildReason};

const SYSTEM_TOOLCHAIN: &[&str] = &[
    "gcc", "glibc", "binutils", "make", "bison", "flex", "perl", "python", "texinfo", "m4", "sed",
    "gawk",
];

pub(super) fn compute_session_hash(build_set: &HashSet<String>) -> String {
    use sha2::{Digest, Sha256};
    let mut names: Vec<&str> = build_set.iter().map(|s| s.as_str()).collect();
    names.sort();
    let mut hasher = Sha256::new();
    for name in &names {
        hasher.update(name.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn expand_missing_dependencies(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
    mode: DependencyMode,
    include_runtime: bool,
    max_depth: usize,
) -> Result<()> {
    let mut build_set: HashSet<String> = HashSet::new();
    let mut traversal_seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    for path in plans_to_build.iter() {
        if let Ok(m) = PlanManifest::from_file(path) {
            build_set.insert(m.plan.name.clone());
            traversal_seen.insert(m.plan.name.clone());
            queue.push_back((m.plan.name.clone(), 0));
        }
    }

    while let Some((name, depth)) = queue.pop_front() {
        let Some(path) = all_plans.get(&name) else {
            continue;
        };
        let manifest = PlanManifest::from_file(path)?;

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
            let dep_depth = depth + 1;

            if dep_depth > max_depth {
                continue;
            }

            if traversal_seen.insert(dep_name.clone()) {
                queue.push_back((dep_name.clone(), dep_depth));
            }

            if matches!(mode, DependencyMode::All) && SYSTEM_TOOLCHAIN.contains(&dep_name.as_str())
            {
                continue;
            }

            if !build_set.contains(&dep_name)
                && dependency_requires_build(&dep_name, all_plans, db, mode)?
            {
                if let Some(plan_path) = all_plans.get(&dep_name) {
                    info!(
                        "Scheduling dependency (depth {}, reason: {}): {}",
                        dep_depth,
                        dependency_reason_label(&dep_name, all_plans, db, mode)?,
                        dep_name,
                    );
                    plans_to_build.insert(plan_path.clone());
                    build_set.insert(dep_name.clone());
                }
            }
        }

        if !matches!(mode, DependencyMode::All) {
            for build_dep in &manifest.dependencies.build {
                let build_dep_name = version::parse_dependency(build_dep)
                    .unwrap_or_else(|_| (build_dep.clone(), None))
                    .0;
                let build_dep_depth = depth + 1;
                if build_dep_depth >= max_depth {
                    continue;
                }

                let mut runtime_queue = VecDeque::new();
                runtime_queue.push_back((build_dep_name.clone(), build_dep_depth));
                let mut runtime_seen = HashSet::new();
                runtime_seen.insert(build_dep_name.clone());

                while let Some((cur, cur_depth)) = runtime_queue.pop_front() {
                    let Some(cur_plan_path) = all_plans.get(&cur) else {
                        continue;
                    };
                    let cur_manifest = match PlanManifest::from_file(cur_plan_path) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };

                    for rdep in &cur_manifest.dependencies.runtime {
                        let rdep_name = version::parse_dependency(rdep)
                            .unwrap_or_else(|_| (rdep.clone(), None))
                            .0;
                        if !runtime_seen.insert(rdep_name.clone()) {
                            continue;
                        }

                        let rdep_depth = cur_depth + 1;
                        if rdep_depth > max_depth {
                            continue;
                        }

                        if traversal_seen.insert(rdep_name.clone()) {
                            queue.push_back((rdep_name.clone(), rdep_depth));
                        }

                        if !build_set.contains(&rdep_name)
                            && dependency_requires_build(&rdep_name, all_plans, db, mode)?
                        {
                            if let Some(rdep_plan_path) = all_plans.get(&rdep_name) {
                                info!(
                                    "Scheduling transitive runtime dependency of {} (depth {}, reason: {}): {}",
                                    build_dep_name,
                                    rdep_depth,
                                    dependency_reason_label(&rdep_name, all_plans, db, mode)?,
                                    rdep_name,
                                );
                                plans_to_build.insert(rdep_plan_path.clone());
                                build_set.insert(rdep_name.clone());
                            }
                        }

                        runtime_queue.push_back((rdep_name, rdep_depth));
                    }
                }
            }
        }
    }

    Ok(())
}

fn dependency_reason_label(
    dep_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &Database,
    mode: DependencyMode,
) -> Result<&'static str> {
    match mode {
        DependencyMode::All => Ok("--deps=all"),
        DependencyMode::Missing => Ok("missing"),
        DependencyMode::Sync => {
            if dependency_plan_differs(dep_name, all_plans, db)? {
                Ok("outdated")
            } else {
                Ok("missing")
            }
        }
        DependencyMode::None => Ok("skipped"),
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

pub(super) fn installed_matches_manifest(
    installed: &InstalledPart,
    manifest: &PlanManifest,
) -> bool {
    installed.epoch == manifest.plan.epoch
        && installed.version == manifest.plan.version
        && installed.release == manifest.plan.release
}

pub(super) fn construction_plan_order(
    build_set: &HashSet<String>,
    deps_map: &HashMap<String, Vec<String>>,
) -> Vec<(String, usize)> {
    let mut indegree: HashMap<String, usize> = build_set
        .iter()
        .map(|name| (name.clone(), 0usize))
        .collect();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for name in build_set {
        let deps = deps_map.get(name).map(Vec::as_slice).unwrap_or(&[]);
        for dep in deps {
            if !build_set.contains(dep) {
                continue;
            }
            *indegree.get_mut(name).expect("build node exists") += 1;
            dependents
                .entry(dep.clone())
                .or_default()
                .push(name.clone());
        }
    }

    let mut depth_map: HashMap<String, usize> = HashMap::new();
    let mut ready = VecDeque::from({
        let mut nodes: Vec<_> = indegree
            .iter()
            .filter_map(|(name, degree)| (*degree == 0).then_some(name.clone()))
            .collect();
        nodes.sort();
        for n in &nodes {
            depth_map.insert(n.clone(), 0);
        }
        nodes
    });
    let mut ordered = Vec::with_capacity(build_set.len());

    while let Some(name) = ready.pop_front() {
        let my_depth = depth_map[&name];
        ordered.push((name.clone(), my_depth));

        let mut next_ready = Vec::new();
        if let Some(children) = dependents.get(&name) {
            for child in children {
                let child_depth = depth_map.entry(child.clone()).or_insert(0);
                *child_depth = (*child_depth).max(my_depth + 1);
                let degree = indegree.get_mut(child).expect("dependent exists");
                *degree -= 1;
                if *degree == 0 {
                    next_ready.push(child.clone());
                }
            }
        }
        next_ready.sort();
        for child in next_ready {
            ready.push_back(child);
        }
    }

    if ordered.len() != build_set.len() {
        let ordered_set: HashSet<_> = ordered.iter().map(|(n, _)| n.clone()).collect();
        let mut remaining: Vec<_> = build_set
            .iter()
            .filter(|name| !ordered_set.contains(*name))
            .cloned()
            .collect();
        remaining.sort();
        for name in remaining {
            ordered.push((name, 0));
        }
    }

    ordered
}

pub(super) fn construction_plan_batches(
    build_set: &HashSet<String>,
    deps_map: &HashMap<String, Vec<String>>,
) -> Vec<(String, usize)> {
    let mut indegree: HashMap<String, usize> = build_set
        .iter()
        .map(|name| (name.clone(), 0usize))
        .collect();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for name in build_set {
        let deps = deps_map.get(name).map(Vec::as_slice).unwrap_or(&[]);
        for dep in deps {
            if !build_set.contains(dep) {
                continue;
            }
            *indegree.get_mut(name).expect("build node exists") += 1;
            dependents
                .entry(dep.clone())
                .or_default()
                .push(name.clone());
        }
    }

    let mut ready: Vec<String> = indegree
        .iter()
        .filter_map(|(name, degree)| (*degree == 0).then_some(name.clone()))
        .collect();
    ready.sort();

    let mut ordered = Vec::with_capacity(build_set.len());
    let mut batch = 0usize;

    while !ready.is_empty() {
        let current_batch = ready;
        ready = Vec::new();

        for name in &current_batch {
            ordered.push((name.clone(), batch));
        }

        for name in current_batch {
            if let Some(children) = dependents.get(&name) {
                let mut next_ready = Vec::new();
                for child in children {
                    let degree = indegree.get_mut(child).expect("dependent exists");
                    *degree -= 1;
                    if *degree == 0 {
                        next_ready.push(child.clone());
                    }
                }
                ready.extend(next_ready);
            }
        }

        ready.sort();
        ready.dedup();
        batch += 1;
    }

    if ordered.len() != build_set.len() {
        let ordered_set: HashSet<_> = ordered.iter().map(|(n, _)| n.clone()).collect();
        let mut remaining: Vec<_> = build_set
            .iter()
            .filter(|name| !ordered_set.contains(*name))
            .cloned()
            .collect();
        remaining.sort();
        for name in remaining {
            ordered.push((name, batch));
        }
    }

    ordered
}

pub(super) fn construction_plan_label(
    name: &str,
    build_set: &HashSet<String>,
    rebuild_reasons: &HashMap<String, RebuildReason>,
    opts: &BuildOptions,
) -> &'static str {
    let is_bootstrap_task = name.ends_with(":bootstrap");
    let is_full_after_bootstrap =
        !is_bootstrap_task && build_set.contains(&format!("{}:bootstrap", name));

    if is_bootstrap_task || opts.mvp {
        "build:mvp"
    } else if is_full_after_bootstrap {
        "build:full"
    } else {
        match rebuild_reasons.get(name) {
            Some(RebuildReason::LinkDependency) => "relink",
            Some(RebuildReason::Transitive) => "rebuild",
            Some(RebuildReason::Explicit) | None => "build",
        }
    }
}

pub(super) fn expand_rebuild_deps(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    rebuild_all: bool,
    max_depth: usize,
    installed_names: &HashSet<String>,
) -> Result<HashMap<String, RebuildReason>> {
    let mut reasons = HashMap::new();

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

    let mut rebuild_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PlanManifest::from_file(path) {
            let name = m.plan.name.clone();
            rebuild_set.insert(name.clone());
            reasons.insert(name, RebuildReason::Explicit);
        }
    }

    let mut current_depth = 0;
    loop {
        if current_depth >= max_depth {
            break;
        }
        let mut wave: Vec<(String, PathBuf, RebuildReason)> = Vec::new();
        for (name, path) in &all_name_to_path {
            if rebuild_set.contains(name) || !installed_names.contains(name) {
                continue;
            }

            let link_changed = link_deps
                .get(name)
                .is_some_and(|deps| deps.iter().any(|d| rebuild_set.contains(d)));

            let other_changed = rebuild_all
                && build_runtime_deps
                    .get(name)
                    .is_some_and(|deps| deps.iter().any(|d| rebuild_set.contains(d)));

            if link_changed || other_changed {
                if !rebuild_all && SYSTEM_TOOLCHAIN.contains(&name.as_str()) {
                    continue;
                }

                let reason = if link_changed {
                    RebuildReason::LinkDependency
                } else {
                    RebuildReason::Transitive
                };
                wave.push((name.clone(), path.clone(), reason));
            }
        }
        if wave.is_empty() {
            break;
        }
        for (name, path, reason) in wave {
            rebuild_set.insert(name.clone());
            plans_to_build.insert(path);
            reasons.insert(name, reason);
        }
        current_depth += 1;
    }

    Ok(reasons)
}

pub(super) fn build_dep_map(
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
