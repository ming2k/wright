use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use tracing::{debug, info};

use crate::error::{Result, WrightError};
use crate::part::version;
use crate::plan::manifest::{PhaseDependencies, PlanManifest};

#[derive(Debug)]
pub(crate) struct PlanGraph {
    pub(crate) name_to_path: HashMap<String, PathBuf>,
    pub(crate) deps_map: HashMap<String, Vec<String>>,
    pub(crate) build_set: HashSet<String>,
    pub(crate) rebuild_reasons: HashMap<String, crate::builder::orchestrator::RebuildReason>,
    pub(crate) pkg_to_plan: HashMap<String, String>,
    /// For bootstrap tasks (key = "{pkg}:bootstrap"), the deps that were
    /// excluded so the cycle could be broken.
    pub(crate) bootstrap_excluded: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub(crate) struct CycleCandidate {
    pub(crate) pkg: String,
    pub(crate) excluded: Vec<String>,
}

pub(crate) fn collect_phase_deps(
    manifest: &PlanManifest,
    pkg_to_plan: &HashMap<String, String>,
    is_mvp: bool,
    all_plans: Option<&HashMap<String, PathBuf>>,
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
    raw_deps.extend(build.clone());
    raw_deps.extend(runtime);
    raw_deps.extend(link);

    for dep in &raw_deps {
        let dep_pkg_name = version::parse_dependency(dep)
            .unwrap_or_else(|_| (dep.clone(), None))
            .0;

        if let Some(parent_plan) = pkg_to_plan.get(&dep_pkg_name) {
            if parent_plan != &manifest.plan.name {
                deps.push(parent_plan.clone());
            }
        } else {
            deps.push(dep_pkg_name);
        }
    }

    // A build dependency is useless unless its full transitive runtime dep tree is
    // installed first. Use BFS to add ordering edges for the entire closure.
    if let Some(plans) = all_plans {
        for build_dep in &build {
            let build_dep_name = version::parse_dependency(build_dep)
                .unwrap_or_else(|_| (build_dep.clone(), None))
                .0;
            let build_dep_plan = pkg_to_plan
                .get(&build_dep_name)
                .cloned()
                .unwrap_or(build_dep_name);

            let mut queue = VecDeque::new();
            queue.push_back(build_dep_plan);
            let mut visited = HashSet::new();

            while let Some(cur) = queue.pop_front() {
                if !visited.insert(cur.clone()) {
                    continue;
                }
                if let Some(plan_path) = plans.get(&cur) {
                    if let Ok(dep_manifest) = PlanManifest::from_file(plan_path) {
                        for rdep in &dep_manifest.dependencies.runtime {
                            let rdep_name = version::parse_dependency(rdep)
                                .unwrap_or_else(|_| (rdep.clone(), None))
                                .0;
                            let rdep_plan =
                                pkg_to_plan.get(&rdep_name).cloned().unwrap_or(rdep_name);
                            if rdep_plan != manifest.plan.name {
                                deps.push(rdep_plan.clone());
                            }
                            queue.push_back(rdep_plan);
                        }
                    }
                }
            }
        }
    }

    deps
}

pub(crate) fn cycle_candidates_for(cycle: &[String], graph: &PlanGraph) -> Vec<CycleCandidate> {
    let cycle_set: HashSet<&str> = cycle.iter().map(|s| s.as_str()).collect();
    let mut candidates = Vec::new();

    for pkg in cycle {
        let path = match graph.name_to_path.get(pkg) {
            Some(p) => p,
            None => continue,
        };
        let manifest = match PlanManifest::from_file(path) {
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

        let full_deps = collect_phase_deps(&manifest, &graph.pkg_to_plan, false, None);
        let mvp_deps = collect_phase_deps(&manifest, &graph.pkg_to_plan, true, None);

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

pub(crate) fn pick_candidate(mut candidates: Vec<CycleCandidate>) -> Option<CycleCandidate> {
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

struct SccState {
    index: usize,
    stack: Vec<String>,
    on_stack: HashMap<String, bool>,
    indices: HashMap<String, usize>,
    lowlinks: HashMap<String, usize>,
    sccs: Vec<Vec<String>>,
}

/// Return all strongly-connected components with more than one node.
pub(crate) fn find_cycles(graph: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
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
            *s.lowlinks
                .get_mut(v)
                .expect("v was inserted at function entry") = s.lowlinks[v].min(ll_w);
        } else if *s.on_stack.get(w.as_str()).unwrap_or(&false) {
            let idx_w = s.indices[w.as_str()];
            *s.lowlinks
                .get_mut(v)
                .expect("v was inserted at function entry") = s.lowlinks[v].min(idx_w);
        }
    }

    if s.lowlinks[v] == s.indices[v] {
        let mut scc = Vec::new();
        loop {
            let w = s
                .stack
                .pop()
                .expect("stack must contain v and its descendants");
            s.on_stack.insert(w.clone(), false);
            scc.push(w.clone());
            if w == v {
                break;
            }
        }
        if scc.len() > 1 {
            s.sccs.push(scc);
        }
    }
}

/// Given an SCC (unordered set of nodes) and the full dependency graph,
/// trace an actual cycle path via DFS and return a display string like
/// "A → B → C → A". Falls back to joining members if no path is found.
pub(crate) fn format_cycle_path(scc: &[String], graph: &HashMap<String, Vec<String>>) -> String {
    let scc_set: HashSet<&str> = scc.iter().map(|s| s.as_str()).collect();

    let start = &scc[0];
    let mut stack: Vec<(&str, Vec<String>)> = vec![(start.as_str(), vec![start.clone()])];
    let mut visited: HashSet<&str> = HashSet::new();

    while let Some((node, path)) = stack.pop() {
        let neighbors = match graph.get(node) {
            Some(n) => n,
            None => continue,
        };
        for neighbor in neighbors {
            if !scc_set.contains(neighbor.as_str()) {
                continue;
            }
            if neighbor == start && path.len() > 1 {
                let mut display = path.clone();
                display.push(start.clone());
                return display.join(" → ");
            }
            if !visited.contains(neighbor.as_str()) {
                visited.insert(neighbor.as_str());
                let mut new_path = path.clone();
                new_path.push(neighbor.clone());
                stack.push((neighbor.as_str(), new_path));
            }
        }
    }

    let mut members = scc.to_vec();
    members.push(scc[0].clone());
    members.join(" → ")
}

/// For each dependency cycle in the graph, find a part with
/// `[mvp.dependencies]` that breaks the cycle and insert a two-pass
/// build plan: `{pkg}:bootstrap` runs first (no cyclic dep), then
/// the rest of the cycle, then `{pkg}` rebuilds fully with all deps.
pub(crate) fn inject_bootstrap_passes(graph: &mut PlanGraph) -> Result<()> {
    let cycles = find_cycles(&graph.deps_map);
    if cycles.is_empty() {
        debug!("Dependency graph is acyclic.");
        return Ok(());
    }

    for cycle in &cycles {
        let cycle_display = format_cycle_path(cycle, &graph.deps_map);
        info!("Detected dependency cycle: {}", cycle_display);

        let candidates = cycle_candidates_for(cycle, graph);
        let chosen = pick_candidate(candidates.clone());

        let (pkg, excl) = match chosen {
            Some(c) => (c.pkg, c.excluded),
            None => {
                return Err(WrightError::BuildError(format!(
                    "Dependency cycle cannot be automatically resolved.\n\
                     Cycle: {}\n\
                     Add inline '[mvp.dependencies]' or a sibling 'mvp.toml' in one of these \
                     plans to declare an acyclic MVP dependency set.",
                    cycle_display
                )));
            }
        };

        let bootstrap_key = format!("{}:bootstrap", pkg);

        let mvp_manifest = PlanManifest::from_file(&graph.name_to_path[&pkg])?;
        let bootstrap_deps = collect_phase_deps(&mvp_manifest, &graph.pkg_to_plan, true, None);

        graph.deps_map.insert(bootstrap_key.clone(), bootstrap_deps);
        graph.build_set.insert(bootstrap_key.clone());
        graph
            .name_to_path
            .insert(bootstrap_key.clone(), graph.name_to_path[&pkg].clone());
        graph
            .bootstrap_excluded
            .insert(bootstrap_key.clone(), excl.clone());

        if let Some(deps) = graph.deps_map.get_mut(&pkg) {
            deps.push(bootstrap_key.clone());
        }

        for other in cycle {
            if other == &pkg {
                continue;
            }
            if let Some(deps) = graph.deps_map.get_mut(other) {
                for dep in deps.iter_mut() {
                    if dep == &pkg {
                        *dep = bootstrap_key.clone();
                    }
                }
            }
        }

        info!(
            "Scheduling cycle resolution for {}: build:mvp without {}, then build:full",
            pkg,
            excl.join(", ")
        );
    }

    Ok(())
}
