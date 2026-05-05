use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use tracing::info;

use crate::builder::mvp::{collect_phase_deps, PlanGraph};
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::part::version;
use crate::plan::manifest::{OutputConfig, PlanManifest};

use super::{BuildOptions, DependentsMode, MatchPolicy, RebuildReason};

pub(super) async fn expand_missing_dependencies(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    db: &InstalledDb,
    policies: &[MatchPolicy],
    domain: DependentsMode,
    max_depth: usize,
    stable_toolchain: &[String],
) -> Result<()> {
    let mut build_set: HashSet<String> = HashSet::new();
    let mut traversal_seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    let mut manifest_cache: HashMap<String, PlanManifest> = HashMap::new();

    for path in plans_to_build.iter() {
        if let Ok(m) = PlanManifest::from_file(path) {
            let name = m.metadata.name.clone();
            build_set.insert(name.clone());
            traversal_seen.insert(name.clone());
            queue.push_back((name.clone(), 0));
            manifest_cache.insert(name, m);
        }
    }

    while let Some((name, depth)) = queue.pop_front() {
        let Some(path) = all_plans.get(&name) else {
            continue;
        };

        if !manifest_cache.contains_key(&name) {
            let m = PlanManifest::from_file(path)?;
            manifest_cache.insert(name.clone(), m);
        }
        let manifest = manifest_cache.get(&name).unwrap();

        let mut deps_to_check: Vec<String> = Vec::new();
        if matches!(domain, DependentsMode::All | DependentsMode::Build) {
            deps_to_check.extend(manifest.build_deps.iter().cloned());
        }
        if matches!(domain, DependentsMode::All | DependentsMode::Link) {
            deps_to_check.extend(manifest.link_deps.iter().cloned());
        }
        if matches!(domain, DependentsMode::All | DependentsMode::Runtime) {
            deps_to_check.extend(manifest.runtime_deps.iter().cloned());
        }

        for dep in &deps_to_check {
            let dep_name = version::parse_dependency(dep)
                .unwrap_or_else(|_| (dep.clone(), None))
                .0;
            let (dep_plan_name, dep_output_name) = version::parse_dep_ref(&dep_name);
            let dep_depth = depth + 1;

            if dep_depth > max_depth {
                continue;
            }

            if traversal_seen.insert(dep_plan_name.clone()) {
                queue.push_back((dep_plan_name.clone(), dep_depth));
            }

            if policies.contains(&MatchPolicy::All)
                && stable_toolchain.iter().any(|t| t == &dep_plan_name)
            {
                continue;
            }

            if !build_set.contains(&dep_plan_name) {
                if let Some(label) = dependency_match_label(
                    &dep_output_name,
                    &dep_plan_name,
                    all_plans,
                    db,
                    policies,
                )
                .await?
                {
                    if let Some(plan_path) = all_plans.get(&dep_plan_name) {
                        info!(
                            "Scheduling dependency (depth {}, reason: {}): {}",
                            dep_depth, label, dep_plan_name,
                        );
                        plans_to_build.insert(plan_path.clone());
                        build_set.insert(dep_plan_name.clone());
                    }
                }
            }
        }

        if !policies.contains(&MatchPolicy::All)
            && matches!(domain, DependentsMode::All | DependentsMode::Build)
        {
            let build_deps: Vec<String> = manifest_cache
                .get(&name)
                .map(|m| m.build_deps.clone())
                .unwrap_or_default();

            for build_dep in &build_deps {
                let build_dep_name = version::parse_dependency(build_dep)
                    .unwrap_or_else(|_| (build_dep.clone(), None))
                    .0;
                let (build_dep_plan_name, _) = version::parse_dep_ref(&build_dep_name);
                let build_dep_depth = depth + 1;
                if build_dep_depth >= max_depth {
                    continue;
                }

                let mut runtime_queue = VecDeque::new();
                runtime_queue.push_back((build_dep_plan_name.clone(), build_dep_depth));
                let mut runtime_seen = HashSet::new();
                runtime_seen.insert(build_dep_plan_name.clone());

                while let Some((cur, cur_depth)) = runtime_queue.pop_front() {
                    let Some(cur_plan_path) = all_plans.get(&cur) else {
                        continue;
                    };

                    if !manifest_cache.contains_key(&cur) {
                        match PlanManifest::from_file(cur_plan_path) {
                            Ok(m) => {
                                manifest_cache.insert(cur.clone(), m);
                            }
                            Err(_) => continue,
                        }
                    }
                    let cur_manifest = manifest_cache.get(&cur).unwrap();

                    for rdep in cur_manifest.runtime_deps.clone() {
                        let rdep_name = version::parse_dependency(&rdep)
                            .unwrap_or_else(|_| (rdep.clone(), None))
                            .0;
                        let (rdep_plan_name, rdep_output_name) = version::parse_dep_ref(&rdep_name);
                        if !runtime_seen.insert(rdep_plan_name.clone()) {
                            continue;
                        }

                        let rdep_depth = cur_depth + 1;
                        if rdep_depth > max_depth {
                            continue;
                        }

                        if traversal_seen.insert(rdep_plan_name.clone()) {
                            queue.push_back((rdep_plan_name.clone(), rdep_depth));
                        }

                        if !build_set.contains(&rdep_plan_name) {
                            if let Some(label) = dependency_match_label(
                                &rdep_output_name,
                                &rdep_plan_name,
                                all_plans,
                                db,
                                policies,
                            )
                            .await?
                            {
                                if let Some(rdep_plan_path) = all_plans.get(&rdep_plan_name) {
                                    info!(
                                        "Scheduling transitive runtime dependency of {} (depth {}, reason: {}): {}",
                                        build_dep_plan_name, rdep_depth, label, rdep_plan_name,
                                    );
                                    plans_to_build.insert(rdep_plan_path.clone());
                                    build_set.insert(rdep_plan_name.clone());
                                }
                            }
                        }

                        runtime_queue.push_back((rdep_plan_name, rdep_depth));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Returns the match reason label when the dependency matches any policy, or `None` if it doesn't.
/// Combines the match check and label derivation into a single database round-trip.
async fn dependency_match_label(
    dep_output_name: &str,
    dep_plan_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &InstalledDb,
    policies: &[MatchPolicy],
) -> Result<Option<&'static str>> {
    let installed = db.get_part(dep_output_name).await?;
    for policy in policies {
        let label = match policy {
            MatchPolicy::All => Some("--match=all"),
            MatchPolicy::Missing => {
                if installed.is_none() {
                    Some("missing")
                } else {
                    None
                }
            }
            MatchPolicy::Installed => {
                if installed.is_some()
                    && !dependency_plan_differs(dep_output_name, dep_plan_name, all_plans, db)
                        .await?
                {
                    Some("installed")
                } else {
                    None
                }
            }
            MatchPolicy::Outdated => {
                if installed.is_none() {
                    Some("missing")
                } else if dependency_plan_differs(dep_output_name, dep_plan_name, all_plans, db)
                    .await?
                {
                    Some("outdated")
                } else {
                    None
                }
            }
        };
        if label.is_some() {
            return Ok(label);
        }
    }
    Ok(None)
}

pub(super) async fn dependency_matches_policy(
    dep_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &InstalledDb,
    policies: &[MatchPolicy],
) -> Result<bool> {
    // 首先尝试直接用 dep_name 查询（单 output plan 或恰好有同名的 output）
    if dependency_match_label(dep_name, dep_name, all_plans, db, policies)
        .await?
        .is_some()
    {
        return Ok(true);
    }

    // 如果是多 output plan，检查是否有任何 output 匹配
    if let Some(plan_path) = all_plans.get(dep_name) {
        if let Ok(manifest) = PlanManifest::from_file(plan_path) {
            if let Some(OutputConfig::Multi(ref outputs)) = manifest.outputs {
                for (output_name, _) in outputs {
                    if dependency_match_label(output_name, dep_name, all_plans, db, policies)
                        .await?
                        .is_some()
                    {
                        return Ok(true);
                    }
                }
            }
        }
    }

    Ok(false)
}

async fn dependency_plan_differs(
    dep_output_name: &str,
    dep_plan_name: &str,
    all_plans: &HashMap<String, PathBuf>,
    db: &InstalledDb,
) -> Result<bool> {
    let Some(installed) = db.get_part(dep_output_name).await? else {
        return Ok(true);
    };

    // Assumed parts are explicitly declared as externally provided.
    // They have no local build plan to compare against, so treat them as
    // up-to-date — wright should never auto-schedule rebuilds for them.
    if installed.origin == crate::database::Origin::External {
        return Ok(false);
    }

    let Some(plan_path) = all_plans.get(dep_plan_name) else {
        return Ok(false);
    };
    let manifest = PlanManifest::from_file(plan_path)?;

    let Some(plan) = db.get_plan_by_id(installed.plan_id).await? else {
        return Ok(true);
    };
    let manifest_ver = manifest.metadata.version.as_deref().unwrap_or("");
    Ok(plan.epoch != manifest.metadata.epoch as i64
        || plan.version != manifest_ver
        || plan.release != manifest.metadata.release as i64)
}

pub(super) fn construction_plan_batches(
    build_set: &HashSet<String>,
    deps_map: &HashMap<String, Vec<String>>,
) -> Result<Vec<(String, usize)>> {
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
        let mut cycle_nodes: Vec<_> = build_set
            .iter()
            .filter(|name| !ordered_set.contains(*name))
            .cloned()
            .collect();
        cycle_nodes.sort();
        return Err(WrightError::BuildError(format!(
            "dependency cycle detected among: {}",
            cycle_nodes.join(", ")
        )));
    }

    Ok(ordered)
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

pub(super) async fn expand_rebuild_deps(
    plans_to_build: &mut HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    mode: DependentsMode,
    max_depth: usize,
    installed_names: &HashSet<String>,
    stable_toolchain: &[String],
) -> Result<HashMap<String, RebuildReason>> {
    let mut reasons = HashMap::new();

    let mut runtime_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut build_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut link_deps: HashMap<String, Vec<String>> = HashMap::new();
    let mut all_name_to_path: HashMap<String, PathBuf> = HashMap::new();

    for (plan_name, plan_path) in all_plans {
        if let Ok(m) = PlanManifest::from_file(plan_path) {
            let r_deps: Vec<String> = m
                .runtime_deps
                .iter()
                .map(|d| {
                    let name = version::parse_dependency(d)
                        .unwrap_or_else(|_| (d.to_string(), None))
                        .0;
                    version::parse_dep_ref(&name).0
                })
                .collect();
            let b_deps: Vec<String> = m
                .build_deps
                .iter()
                .map(|d| {
                    let name = version::parse_dependency(d)
                        .unwrap_or_else(|_| (d.to_string(), None))
                        .0;
                    version::parse_dep_ref(&name).0
                })
                .collect();
            let l_deps: Vec<String> = m
                .link_deps
                .iter()
                .map(|d| {
                    let name = version::parse_dependency(d)
                        .unwrap_or_else(|_| (d.to_string(), None))
                        .0;
                    version::parse_dep_ref(&name).0
                })
                .collect();

            runtime_deps.insert(plan_name.clone(), r_deps);
            build_deps.insert(plan_name.clone(), b_deps);
            link_deps.insert(plan_name.clone(), l_deps);
            all_name_to_path.insert(plan_name.clone(), plan_path.clone());
        }
    }

    let mut rebuild_set: HashSet<String> = HashSet::new();
    for path in plans_to_build.iter() {
        if let Ok(m) = PlanManifest::from_file(path) {
            let name = m.metadata.name.clone();
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

            let link_changed = matches!(mode, DependentsMode::All | DependentsMode::Link)
                && link_deps
                    .get(name)
                    .is_some_and(|deps| deps.iter().any(|d| rebuild_set.contains(d)));

            let runtime_changed = matches!(mode, DependentsMode::All | DependentsMode::Runtime)
                && runtime_deps
                    .get(name)
                    .is_some_and(|deps| deps.iter().any(|d| rebuild_set.contains(d)));

            let build_changed = matches!(mode, DependentsMode::All | DependentsMode::Build)
                && build_deps
                    .get(name)
                    .is_some_and(|deps| deps.iter().any(|d| rebuild_set.contains(d)));

            if link_changed || runtime_changed || build_changed {
                if !matches!(mode, DependentsMode::All)
                    && stable_toolchain.iter().any(|t| t == name)
                {
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

    let mut part_to_plan = HashMap::new();
    for (plan_name, path) in all_plans {
        part_to_plan.insert(plan_name.clone(), plan_name.clone());
        if let Ok(m) = PlanManifest::from_file(path) {
            if let Some(OutputConfig::Multi(ref parts)) = m.outputs {
                for (sub_name, _) in parts {
                    part_to_plan.insert(sub_name.clone(), plan_name.clone());
                }
            }
        }
    }

    for path in plans_to_build {
        let manifest = PlanManifest::from_file(path)?;
        let name = manifest.metadata.name.clone();
        name_to_path.insert(name.clone(), path.clone());
        build_set.insert(name.clone());

        let mut deps = Vec::new();
        if !checksum {
            deps = collect_phase_deps(&manifest, &part_to_plan, is_mvp, Some(all_plans));

            if is_mvp {
                let full_deps =
                    collect_phase_deps(&manifest, &part_to_plan, false, Some(all_plans));
                let mvp_deps = collect_phase_deps(&manifest, &part_to_plan, true, Some(all_plans));
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
        part_to_plan,
        bootstrap_excluded,
    })
}
