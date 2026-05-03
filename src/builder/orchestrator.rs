//! Build orchestrator — parallel build scheduling, dependency resolution,
//! cascade expansion, and automatic bootstrap cycle detection/resolution.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

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

use execute::execute_builds;
use planning::{
    build_dep_map, construction_plan_batches, construction_plan_label, expand_missing_dependencies,
    expand_rebuild_deps,
};
use resolver::resolve_targets;

pub use resolver::setup_resolver;

use crate::archive::resolver::LocalResolver;
use crate::plan::manifest::OutputConfig;

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
    pub until_stage: Option<String>,
    pub fetch_only: bool,
    pub clean: bool,
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
        !self.checksum && !self.fetch_only
    }
}

fn plan_file_fingerprint(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    hasher.update(canonical.to_string_lossy().as_bytes());
    hasher.update(b"\n-- plan.toml --\n");
    let content = std::fs::read(path).map_err(|e| {
        WrightError::BuildError(format!(
            "failed to read {} for session hash: {}",
            path.display(),
            e
        ))
    })?;
    hasher.update(&content);

    let mvp_path = path.with_file_name("mvp.toml");
    if mvp_path.exists() {
        hasher.update(b"\n-- mvp.toml --\n");
        let mvp_content = std::fs::read(&mvp_path).map_err(|e| {
            WrightError::BuildError(format!(
                "failed to read {} for session hash: {}",
                mvp_path.display(),
                e
            ))
        })?;
        hasher.update(&mvp_content);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn task_fingerprints(plan: &BuildExecutionPlan) -> Result<BTreeMap<String, String>> {
    let mut fingerprints = BTreeMap::new();
    let mut tasks: Vec<_> = plan.name_to_path.iter().collect();
    tasks.sort_by(|a, b| a.0.cmp(b.0));

    for (task_name, path) in tasks {
        fingerprints.insert(task_name.clone(), plan_file_fingerprint(path)?);
    }

    Ok(fingerprints)
}

pub fn compute_build_session_hash(
    plan: &BuildExecutionPlan,
    opts: &BuildOptions,
    session_scope: &str,
) -> Result<String> {
    use sha2::{Digest, Sha256};

    let fingerprints = task_fingerprints(plan)?;
    let mut hasher = Sha256::new();
    hasher.update(b"wright-session-v2\n");
    hasher.update(session_scope.as_bytes());
    hasher.update(b"\n");
    hasher.update(format!("force={}\n", opts.force).as_bytes());
    hasher.update(format!("clean={}\n", opts.clean).as_bytes());
    hasher.update(format!("mvp={}\n", opts.mvp).as_bytes());
    hasher.update(format!("fetch_only={}\n", opts.fetch_only).as_bytes());
    hasher.update(format!("skip_check={}\n", opts.skip_check).as_bytes());
    hasher.update(format!("checksum={}\n", opts.checksum).as_bytes());
    hasher.update(format!("until_stage={:?}\n", opts.until_stage).as_bytes());
    hasher.update(format!("stages={:?}\n", opts.stages).as_bytes());

    for (batch_idx, tasks) in plan.batches.iter().enumerate() {
        hasher.update(format!("batch:{}\n", batch_idx).as_bytes());
        let mut batch_tasks = tasks.clone();
        batch_tasks.sort();
        for task in batch_tasks {
            hasher.update(task.as_bytes());
            hasher.update(b"\n");
            if let Some(fingerprint) = fingerprints.get(&task) {
                hasher.update(fingerprint.as_bytes());
                hasher.update(b"\n");
            }
        }
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn expected_task_artifacts(parts_dir: &Path, plan_path: &Path) -> Result<Vec<PathBuf>> {
    let manifest = PlanManifest::from_file(plan_path)?;
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => Ok(parts
            .iter()
            .map(|(sub_name, sub_part)| {
                let sub_manifest = sub_part.to_manifest(sub_name, &manifest);
                parts_dir.join(sub_manifest.part_filename())
            })
            .collect()),
        _ => Ok(vec![parts_dir.join(manifest.part_filename())]),
    }
}

pub async fn load_completed_build_tasks(
    db: &InstalledDb,
    config: &GlobalConfig,
    plan: &BuildExecutionPlan,
    session_hash: &str,
) -> Result<HashSet<String>> {
    let completed = db
        .get_execution_session_completed_items(session_hash)
        .await?;
    let mut reusable = HashSet::new();

    for task_name in completed {
        let Some(plan_path) = plan.name_to_path.get(&task_name) else {
            continue;
        };
        let artifacts = expected_task_artifacts(&config.general.parts_dir, plan_path)?;
        if artifacts.iter().all(|path| path.exists()) {
            reusable.insert(task_name);
        }
    }

    Ok(reusable)
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
        let db = InstalledDb::open(&db_path)
            .await
            .context("failed to open database for dependency resolution")?;

        if let Some(domain) = opts.deps {
            expand_missing_dependencies(
                &mut plans_to_build,
                &all_plans,
                &db,
                &opts.match_policies,
                domain,
                actual_max,
            )
            .await?;
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

        if let Some(domain) = opts.rdeps {
            let installed_names: HashSet<String> = db
                .list_parts()
                .await
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
        session_hash: Option<&str>,
        session_completed: &HashSet<String>,
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
            session_hash,
            session_completed,
        )
        .await
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

pub fn lint_dependency_graph_for_targets(
    config: &GlobalConfig,
    targets: &[String],
) -> Result<()> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let plans_to_build = resolve_targets(targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Ok(());
    }

    let graph = planning::build_dep_map(
        &plans_to_build,
        false,
        false,
        HashMap::new(),
        &all_plans,
    )?;

    execute::lint_dependency_graph(&graph)
}

pub async fn run_build(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: BuildOptions,
) -> Result<()> {
    let plan = create_execution_plan(config, targets, &opts)?;

    let session_hash = if opts.resume.is_some() || opts.is_build_op() {
        Some(compute_build_session_hash(&plan, &opts, "build")?)
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
                let db = InstalledDb::open(&config.general.installed_db_path)
                    .await
                    .context("failed to open database for resume")?;
                if let Some(session) = db.get_execution_session(&hash).await? {
                    if session.command_kind != "build" {
                        return Err(WrightError::BuildError(format!(
                            "session {} is for '{}', not 'build'",
                            &hash[..12.min(hash.len())],
                            session.command_kind
                        )));
                    }
                    let completed = load_completed_build_tasks(
                        &db,
                        config,
                        &plan,
                        session.task_session_hash.as_deref().unwrap_or(&hash),
                    )
                    .await?;
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
                let _ = db
                    .ensure_execution_session(hash, "build", Some(hash), None)
                    .await;
                let _ = db.ensure_execution_session_items(hash, &packages).await;
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
    )
    .await;

    match &result {
        Ok(()) => {
            if let Some(ref hash) = active_session_hash {
                if let Ok(db) = InstalledDb::open(&config.general.installed_db_path).await {
                    let _ = db.clear_execution_session(hash).await;
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
