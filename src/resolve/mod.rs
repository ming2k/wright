//! Target resolution — the first step of a Delivery.
//!
//! Resolve discovers plan files, builds a name→path index, resolves user
//! targets to canonical `plan.toml` paths, expands dependency closures, and
//! constructs a `ForgeExecutionPlan` — the batched DAG that `forge` executes.
//!
//! This is step 1 of the four-step Delivery flow: resolve → forge → seal → deploy.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError, WrightResultExt};
use crate::forge::logging;
use crate::forge::mvp::inject_bootstrap_passes;
use tracing::info;

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::plan::manifest::PlanManifest;

mod graph;
mod resolver;

use graph::{
    build_dep_map, construction_plan_batches, construction_plan_label, expand_missing_dependencies,
    expand_rebuild_deps,
};

pub use resolver::{plan_search_dirs, resolve_targets, setup_part_store};

#[derive(Debug, Clone)]
pub struct ForgeExecutionPlan {
    name_to_path: HashMap<String, PathBuf>,
    deps_map: HashMap<String, Vec<String>>,
    build_set: HashSet<String>,
    bootstrap_excluded: HashMap<String, Vec<String>>,
    rebuild_reasons: HashMap<String, RebuildReason>,
    batches: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgeResourceSummary {
    pub total_cpus: usize,
    pub concurrent_tasks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchPolicy {
    Missing,
    Outdated,
    Installed,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DependentsMode {
    #[default]
    None,
    Link,
    Runtime,
    Forge,
    All,
}

/// Bit-flag domain for selecting which dependency fields to traverse.
///
/// Used by dependency resolution to flexibly combine build, link, and
/// runtime dependency expansion.  Multiple domains can be OR'd together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepDomain(u8);

impl Default for DepDomain {
    fn default() -> Self {
        Self::empty()
    }
}

impl DepDomain {
    pub const BUILD: Self = Self(1 << 0);
    pub const LINK: Self = Self(1 << 1);
    pub const RUNTIME: Self = Self(1 << 2);
    pub const ALL: Self = Self(0b111);

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn from_dependents_mode(mode: DependentsMode) -> Self {
        match mode {
            DependentsMode::None => Self::empty(),
            DependentsMode::Link => Self::LINK,
            DependentsMode::Runtime => Self::RUNTIME,
            DependentsMode::Forge => Self::BUILD,
            DependentsMode::All => Self::ALL,
        }
    }

    pub fn from_modes(modes: &[DependentsMode]) -> Self {
        let mut result = Self::empty();
        for mode in modes {
            result.insert(Self::from_dependents_mode(*mode));
        }
        result
    }
}

/// Options for dependency/dependent resolution via `wright resolve`.
#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    pub deps: DepDomain,
    pub rdeps: DepDomain,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub include_targets: bool,
    pub preserve_targets: bool,
}

/// Options for a build run.
#[derive(Debug, Clone, Default)]
pub struct ForgeOptions {
    pub stages: Vec<String>,
    pub force_stage: Vec<String>,
    pub until_stage: Option<String>,
    pub fetch_only: bool,
    pub clean: bool,
    pub force: bool,
    pub checksum: bool,
    pub skip_check: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub mvp: bool,
    pub nproc_per_isolation: Option<u32>,
}

impl ForgeOptions {
    fn is_build_op(&self) -> bool {
        !self.checksum && !self.fetch_only
    }
}

pub fn plan_file_fingerprint(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    hasher.update(canonical.to_string_lossy().as_bytes());
    hasher.update(b"\n-- plan.toml --\n");
    let content = std::fs::read(path).map_err(|e| {
        WrightError::ForgeError(format!(
            "failed to read {} for plan fingerprint: {}",
            path.display(),
            e
        ))
    })?;
    hasher.update(&content);

    let mvp_path = path.with_file_name("mvp.toml");
    if mvp_path.exists() {
        hasher.update(b"\n-- mvp.toml --\n");
        let mvp_content = std::fs::read(&mvp_path).map_err(|e| {
            WrightError::ForgeError(format!(
                "failed to read {} for plan fingerprint: {}",
                mvp_path.display(),
                e
            ))
        })?;
        hasher.update(&mvp_content);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn resolve_explicit_plan_names(
    plan_dirs: &[PathBuf],
    targets: &[String],
) -> Result<HashSet<String>> {
    let index = crate::plan::discovery::PlanIndex::discover(plan_dirs)?;
    let paths = resolve_targets(targets, &index, plan_dirs)?;
    Ok(paths
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| m.metadata.name)
        .collect())
}

pub async fn resolve_build_set(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: ResolveOptions,
) -> Result<Vec<String>> {
    let plan_dirs = plan_search_dirs(config);
    let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
    let plans_to_build = resolve_targets(&targets, &index, &plan_dirs)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::ForgeError(
            "No targets found matching the requested names.".to_string(),
        ));
    }

    let plans_to_build: HashSet<PathBuf> = plans_to_build
        .into_iter()
        .filter_map(|p| p.canonicalize().ok().or(Some(p)))
        .collect();
    let original_plans = plans_to_build.clone();
    let mut plans_to_build = original_plans.clone();
    let actual_max = {
        let max_depth = opts.depth.unwrap_or(1);
        if max_depth == 0 {
            usize::MAX
        } else {
            max_depth
        }
    };

    {
        let db_path = config.general.db_path.clone();
        let db = InstalledDb::open(&db_path)
            .await
            .context("failed to open database for dependency resolution")?;

        if opts.deps.contains(DepDomain::ALL) {
            let dep_count = expand_missing_dependencies(
                &mut plans_to_build,
                &index,
                &db,
                &opts.match_policies,
                opts.deps,
                actual_max,
                &config.build.stable_toolchain,
            )
            .await?;
            if dep_count > 0 {
                info!(
                    "resolved {} {}",
                    dep_count,
                    if dep_count == 1 {
                        "dependency"
                    } else {
                        "dependencies"
                    }
                );
            }
        }

        if !opts.match_policies.contains(&MatchPolicy::All) {
            let mut retained = HashSet::new();
            for path in plans_to_build {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if opts.preserve_targets && original_plans.contains(&canonical) {
                    retained.insert(path);
                    continue;
                }
                if let Ok(m) = PlanManifest::from_file(&path) {
                    if graph::dependency_matches_policy(
                        &m.metadata.name,
                        &index,
                        &db,
                        &opts.match_policies,
                    )
                    .await
                    .unwrap_or(true)
                    {
                        retained.insert(path);
                    }
                } else {
                    retained.insert(path);
                }
            }
            plans_to_build = retained;
        }

        if opts.rdeps.contains(DepDomain::ALL) {
            let installed_names: HashSet<String> = db
                .list_parts()
                .await
                .context("failed to list deployed parts for dependents filter")?
                .into_iter()
                .map(|p| p.name)
                .collect();
            expand_rebuild_deps(
                &mut plans_to_build,
                &index,
                opts.rdeps,
                actual_max,
                &installed_names,
                &config.build.stable_toolchain,
            )
            .await?;
        }
    }

    if !opts.include_targets {
        plans_to_build.retain(|p| !original_plans.contains(p));
    }

    let names: Vec<String> = plans_to_build
        .iter()
        .map(|p| {
            PlanManifest::from_file(p)
                .map(|m| m.metadata.name)
                .context(format!("failed to parse plan file: {}", p.display()))
        })
        .collect::<Result<Vec<String>>>()?;

    Ok(names)
}

pub fn create_execution_plan(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: &ForgeOptions,
    dep_domain: DepDomain,
) -> Result<ForgeExecutionPlan> {
    let plan_dirs = plan_search_dirs(config);
    let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
    let plans_to_build = resolve_targets(&targets, &index, &plan_dirs)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::ForgeError(
            "No targets specified to build.".to_string(),
        ));
    }

    let reasons: HashMap<String, RebuildReason> = plans_to_build
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| (m.metadata.name, RebuildReason::Explicit))
        .collect();

    let mut graph = build_dep_map(
        &plans_to_build,
        opts.checksum,
        opts.mvp,
        reasons,
        &index,
        dep_domain,
    )?;

    if opts.is_build_op() && !opts.mvp {
        inject_bootstrap_passes(&mut graph)?;
    }

    let mut grouped_batches: Vec<Vec<String>> = Vec::new();
    for (name, batch) in construction_plan_batches(&graph.build_set, &graph.deps_map)? {
        if grouped_batches.len() <= batch {
            grouped_batches.resize_with(batch + 1, Vec::new);
        }
        grouped_batches[batch].push(name);
    }

    Ok(ForgeExecutionPlan {
        name_to_path: graph.name_to_path,
        deps_map: graph.deps_map,
        build_set: graph.build_set,
        bootstrap_excluded: graph.bootstrap_excluded,
        rebuild_reasons: graph.rebuild_reasons,
        batches: grouped_batches,
    })
}

impl ForgeExecutionPlan {
    pub fn batches(&self) -> &[Vec<String>] {
        &self.batches
    }

    pub fn plan_path_for_task(&self, task_name: &str) -> Option<&PathBuf> {
        self.name_to_path.get(task_name)
    }

    pub fn label_for_task(&self, task_name: &str, opts: &ForgeOptions) -> &'static str {
        construction_plan_label(task_name, &self.build_set, &self.rebuild_reasons, opts)
    }

    pub fn describe_task(&self, task_name: &str, opts: &ForgeOptions) -> String {
        describe_task_action(task_name, self.label_for_task(task_name, opts))
    }

    pub fn task_base_name(task: &str) -> &str {
        task.trim_end_matches(":bootstrap")
    }

    pub fn build_set(&self) -> &HashSet<String> {
        &self.build_set
    }

    pub fn deps_for_task(&self, task: &str) -> &[String] {
        self.deps_map.get(task).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn bootstrap_excluded_for(&self, task: &str) -> &[String] {
        self.bootstrap_excluded
            .get(task)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn is_post_bootstrap_full(&self, task: &str) -> bool {
        !task.ends_with(":bootstrap") && self.build_set.contains(&format!("{}:bootstrap", task))
    }
}

pub fn summarize_forge_resources(config: &GlobalConfig) -> ForgeResourceSummary {
    let available_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let total_cpus = if let Some(cap) = config.build.max_cpus {
        available_cpus.min(cap.max(1))
    } else {
        available_cpus
    };

    ForgeResourceSummary {
        total_cpus,
        concurrent_tasks: total_cpus,
    }
}

pub fn describe_forge_resources(resources: ForgeResourceSummary) -> String {
    logging::describe_build_capacity(resources.concurrent_tasks, resources.total_cpus)
}

pub fn describe_task_action(task_name: &str, label: &str) -> String {
    let plan_name = ForgeExecutionPlan::task_base_name(task_name);
    match label {
        "build" => format!("forge {}", plan_name),
        "rebuild" => format!("reforge {}", plan_name),
        "relink" => format!("relink {}", plan_name),
        "build:mvp" => format!("bootstrap {}", plan_name),
        "build:full" => format!("full reforge {}", plan_name),
        _ => format!("process {}", plan_name),
    }
}

pub fn describe_batch_actions(
    plan: &ForgeExecutionPlan,
    tasks: &[String],
    opts: &ForgeOptions,
) -> String {
    let mut actions = Vec::with_capacity(tasks.len());
    for task in tasks {
        actions.push(plan.describe_task(task, opts));
    }
    actions.join(", ")
}

pub fn lint_dependency_graph_for_targets(config: &GlobalConfig, targets: &[String]) -> Result<()> {
    let plan_dirs = plan_search_dirs(config);
    let index = crate::plan::discovery::PlanIndex::discover(&plan_dirs)?;
    let plans_to_build = resolve_targets(targets, &index, &plan_dirs)?;

    if plans_to_build.is_empty() {
        return Ok(());
    }

    let graph = graph::build_dep_map(
        &plans_to_build,
        false,
        false,
        HashMap::new(),
        &index,
        DepDomain::ALL,
    )?;

    lint_dependency_graph(&graph)
}

fn lint_dependency_graph(graph: &crate::forge::mvp::PlanGraph) -> Result<()> {
    use crate::forge::mvp::{cycle_candidates_for, find_cycles, format_cycle_path, pick_candidate};
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
        let candidates = cycle_candidates_for(cycle, graph);
        if candidates.is_empty() {
            println!("{} | - | - | no candidates", idx + 1);
            continue;
        }
        let chosen = pick_candidate(candidates.clone());
        for cand in candidates {
            let selected = match &chosen {
                Some(c) if c.part == cand.part && c.excluded == cand.excluded => "yes",
                _ => "no",
            };
            println!(
                "{} | {} | {} | {}",
                idx + 1,
                cand.part,
                cand.excluded.join(", "),
                selected
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    Explicit,
    LinkDependency,
    Transitive,
}

use std::ops::BitOr;

impl BitOr for DepDomain {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}
