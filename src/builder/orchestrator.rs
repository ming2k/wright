//! Build orchestrator — parallel build scheduling, dependency resolution,
//! cascade expansion, and automatic bootstrap cycle detection/resolution.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::builder::logging;
use crate::builder::mvp::inject_bootstrap_passes;
use crate::error::{Result, WrightError, WrightResultExt};
use tracing::{info, warn};

use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::plan::manifest::PlanManifest;

mod execute;
mod planning;
mod resolver;

use execute::{execute_builds, lint_dependency_graph};
use planning::{
    build_dep_map, compute_session_hash, construction_plan_batches, construction_plan_label,
    expand_missing_dependencies, expand_rebuild_deps,
};
use resolver::resolve_targets;

pub use resolver::setup_resolver;

use crate::archive::resolver::LocalResolver;

#[derive(Debug, Clone)]
pub struct BuildExecutionPlan {
    name_to_path: HashMap<String, PathBuf>,
    deps_map: HashMap<String, Vec<String>>,
    build_set: HashSet<String>,
    bootstrap_excluded: HashMap<String, Vec<String>>,
    rebuild_reasons: HashMap<String, RebuildReason>,
    batches: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildResourceSummary {
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
    Build,
    All,
}

/// Options for dependency/dependent resolution via `wright resolve`.
#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    pub deps: Option<DependentsMode>,
    pub rdeps: Option<DependentsMode>,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub include_targets: bool,
    pub preserve_targets: bool,
}

/// Options for a build run.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    pub stages: Vec<String>,
    pub fetch_only: bool,
    pub clean: bool,
    pub lint: bool,
    pub force: bool,
    pub resume: Option<Option<String>>,
    pub checksum: bool,
    pub skip_check: bool,
    pub verbose: bool,
    pub quiet: bool,
    pub mvp: bool,
    pub print_parts: bool,
    pub nproc_per_isolation: Option<u32>,
}

impl BuildOptions {
    fn is_build_op(&self) -> bool {
        !self.checksum && !self.lint && !self.fetch_only
    }
}

pub fn resolve_explicit_plan_names(
    resolver: &LocalResolver,
    targets: &[String],
) -> Result<HashSet<String>> {
    let all_plans = resolver.get_all_plans()?;
    let paths = resolve_targets(targets, &all_plans, resolver)?;
    Ok(paths
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| m.plan.name)
        .collect())
}

pub async fn resolve_build_set(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: ResolveOptions,
) -> Result<Vec<String>> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::BuildError(
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
        let db_path = config.general.installed_db_path.clone();
        let db = InstalledDb::open(&db_path).await
            .context("failed to open database for dependency resolution")?;

        if let Some(domain) = opts.deps {
            expand_missing_dependencies(
                &mut plans_to_build,
                &all_plans,
                &db,
                &opts.match_policies,
                domain,
                actual_max,
            ).await?;
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
                    if crate::builder::orchestrator::planning::dependency_matches_policy(
                        &m.plan.name,
                        &all_plans,
                        &db,
                        &opts.match_policies,
                    ).await.unwrap_or(true) {
                        retained.insert(path);
                    }
                } else {
                    retained.insert(path);
                }
            }
            plans_to_build = retained;
        }

        if let Some(domain) = opts.rdeps {
            let installed_names: HashSet<String> = db
                .list_parts().await
                .context("failed to list installed parts for dependents filter")?
                .into_iter()
                .map(|p| p.name)
                .collect();
            expand_rebuild_deps(
                &mut plans_to_build,
                &all_plans,
                domain,
                actual_max,
                &installed_names,
            ).await?;
        }
    }

    if !opts.include_targets {
        plans_to_build.retain(|p| !original_plans.contains(p));
    }

    let names: Vec<String> = plans_to_build
        .iter()
        .map(|p| {
            PlanManifest::from_file(p)
                .map(|m| m.plan.name)
                .context(format!("failed to parse plan file: {}", p.display()))
        })
        .collect::<Result<Vec<String>>>()?;

    Ok(names)
}

pub fn create_execution_plan(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: &BuildOptions,
) -> Result<BuildExecutionPlan> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::BuildError(
            "No targets specified to build.".to_string(),
        ));
    }

    let reasons: HashMap<String, RebuildReason> = plans_to_build
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| (m.plan.name, RebuildReason::Explicit))
        .collect();

    let mut graph = build_dep_map(
        &plans_to_build,
        opts.checksum,
        opts.mvp,
        reasons,
        &all_plans,
    )?;

    if opts.is_build_op() && !opts.mvp {
        inject_bootstrap_passes(&mut graph)?;
    }

    let mut grouped_batches: Vec<Vec<String>> = Vec::new();
    for (name, batch) in construction_plan_batches(&graph.build_set, &graph.deps_map) {
        if grouped_batches.len() <= batch {
            grouped_batches.resize_with(batch + 1, Vec::new);
        }
        grouped_batches[batch].push(name);
    }

    Ok(BuildExecutionPlan {
        name_to_path: graph.name_to_path,
        deps_map: graph.deps_map,
        build_set: graph.build_set,
        bootstrap_excluded: graph.bootstrap_excluded,
        rebuild_reasons: graph.rebuild_reasons,
        batches: grouped_batches,
    })
}

impl BuildExecutionPlan {
    pub fn batches(&self) -> &[Vec<String>] {
        &self.batches
    }

    pub fn plan_path_for_task(&self, task_name: &str) -> Option<&PathBuf> {
        self.name_to_path.get(task_name)
    }

    pub fn label_for_task(&self, task_name: &str, opts: &BuildOptions) -> &'static str {
        construction_plan_label(task_name, &self.build_set, &self.rebuild_reasons, opts)
    }

    pub fn describe_task(&self, task_name: &str, opts: &BuildOptions) -> String {
        describe_task_action(task_name, self.label_for_task(task_name, opts))
    }

    pub fn task_base_name(task: &str) -> &str {
        task.trim_end_matches(":bootstrap")
    }

    pub async fn execute_batch(
        &self,
        config: &GlobalConfig,
        batch_index: usize,
        opts: &BuildOptions,
    ) -> Result<()> {
        let batch = self.batches.get(batch_index).ok_or_else(|| {
            WrightError::BuildError(format!("unknown build batch {}", batch_index))
        })?;
        let batch_set: HashSet<String> = batch.iter().cloned().collect();
        execute_builds(
            config,
            &self.name_to_path,
            &self.deps_map,
            &batch_set,
            opts,
            &self.bootstrap_excluded,
            None,
            &HashSet::new(),
        ).await
    }
}

pub fn summarize_build_resources(config: &GlobalConfig) -> BuildResourceSummary {
    let available_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let total_cpus = if let Some(cap) = config.build.max_cpus {
        available_cpus.min(cap.max(1))
    } else {
        available_cpus
    };

    BuildResourceSummary {
        total_cpus,
        concurrent_tasks: total_cpus,
    }
}

pub fn describe_build_resources(resources: BuildResourceSummary) -> String {
    logging::describe_build_capacity(resources.concurrent_tasks, resources.total_cpus)
}

pub fn describe_task_action(task_name: &str, label: &str) -> String {
    let plan_name = BuildExecutionPlan::task_base_name(task_name);
    match label {
        "build" => format!("build {}", plan_name),
        "rebuild" => format!("rebuild {}", plan_name),
        "relink" => format!("relink {}", plan_name),
        "build:mvp" => format!("bootstrap {}", plan_name),
        "build:full" => format!("full rebuild {}", plan_name),
        _ => format!("process {}", plan_name),
    }
}

pub fn describe_batch_actions(
    plan: &BuildExecutionPlan,
    tasks: &[String],
    opts: &BuildOptions,
) -> String {
    let mut actions = Vec::with_capacity(tasks.len());
    for task in tasks {
        actions.push(plan.describe_task(task, opts));
    }
    actions.join(", ")
}

pub async fn run_build(config: &GlobalConfig, targets: Vec<String>, opts: BuildOptions) -> Result<()> {
    if opts.lint {
        let resolver = setup_resolver(config)?;
        let all_plans = resolver.get_all_plans()?;
        let plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;
        return lint_dependency_graph(&plans_to_build, &all_plans, build_dep_map);
    }

    let plan = create_execution_plan(config, targets, &opts)?;

    let session_hash = if opts.resume.is_some() || opts.is_build_op() {
        Some(compute_session_hash(&plan.build_set))
    } else {
        None
    };

    let (_effective_resume, session_completed) = match &opts.resume {
        Some(explicit_hash) => {
            let hash = match explicit_hash {
                Some(h) => h.clone(),
                None => session_hash.clone().unwrap_or_default(),
            };
            if hash.is_empty() {
                (false, HashSet::new())
            } else {
                let db = InstalledDb::open(&config.general.installed_db_path).await
                    .context("failed to open database for resume")?;
                if db.session_exists(&hash).await? {
                    let completed = db.get_session_completed(&hash).await?;
                    info!(
                        "resuming session {} ({}/{} completed)",
                        &hash[..12.min(hash.len())],
                        completed.len(),
                        plan.build_set.len()
                    );
                    (true, completed)
                } else {
                    warn!(
                        "no existing session {} found, starting fresh build",
                        &hash[..12.min(hash.len())]
                    );
                    (false, HashSet::new())
                }
            }
        }
        None => (false, HashSet::new()),
    };

    if !opts.quiet {
        let resources = summarize_build_resources(config);
        info!("{}", describe_build_resources(resources));
    }

    if !opts.quiet {
        for (batch, tasks) in plan.batches.iter().enumerate() {
            let pending: Vec<String> = tasks
                .iter()
                .filter(|name| !session_completed.contains(*name))
                .cloned()
                .collect();

            for name in tasks {
                if session_completed.contains(name) {
                    info!(
                        "Skipping batch {}: plan {} was already completed in a previous run.",
                        batch + 1,
                        name.trim_end_matches(":bootstrap"),
                    );
                }
            }

            if !pending.is_empty() {
                info!(
                    "{}",
                    logging::describe_batch(
                        "Build",
                        batch + 1,
                        plan.batches.len(),
                        &describe_batch_actions(&plan, &pending, &opts),
                    ),
                );
            }
        }
    }

    let active_session_hash = if let Some(ref hash) = session_hash {
        if opts.is_build_op() {
            if let Ok(db) = InstalledDb::open(&config.general.installed_db_path).await {
                let packages: Vec<String> = plan.name_to_path.keys().cloned().collect();
                let _ = db.create_session(hash, &packages).await;
            }
            Some(hash.clone())
        } else {
            None
        }
    } else {
        None
    };

    let result = execute_builds(
        config,
        &plan.name_to_path,
        &plan.deps_map,
        &plan.build_set,
        &opts,
        &plan.bootstrap_excluded,
        active_session_hash.as_deref(),
        &session_completed,
    ).await;

    match &result {
        Ok(()) => {
            if let Some(ref hash) = active_session_hash {
                if let Ok(db) = InstalledDb::open(&config.general.installed_db_path).await {
                    let _ = db.clear_session(hash).await;
                }
            }
        }
        Err(_) => {
            if let Some(ref hash) = active_session_hash {
                info!(
                    "Build session saved as {}. Resume with --resume {}.",
                    hash, hash
                );
            }
        }
    }

    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    Explicit,
    LinkDependency,
    Transitive,
}
