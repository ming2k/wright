//! Target resolution — the first step of a Delivery.
//!
//! Resolve discovers plan files, builds a name→path index, resolves user
//! targets to canonical `plan.toml` paths, expands dependency closures, and
//! constructs a `BuildExecutionPlan` — the batched DAG that the build step
//! executes inside the Foundry.
//!
//! This is step 1 of the four-step Delivery flow: resolve → build → seal → deploy.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError, WrightResultExt};
use tracing::info;

use crate::config::GlobalConfig;
use wright_model::version;
use wright_plan::manifest::PlanManifest;
use wright_state::database::InstalledDb;

mod bootstrap;
mod graph;
mod resolver;

use bootstrap::inject_bootstrap_passes;

use graph::{
    build_dep_map, construction_plan_batches, construction_plan_label, expand_missing_dependencies,
    expand_rebuild_deps,
};

pub use resolver::{plan_search_dirs, resolve_targets, setup_part_store};

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
pub struct BuildPlanOptions {
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

impl BuildPlanOptions {
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
        WrightError::context(
            format!("failed to read {} for plan fingerprint", path.display()),
            e,
        )
    })?;
    hasher.update(&content);

    let mvp_path = path.with_file_name("mvp.toml");
    if mvp_path.exists() {
        hasher.update(b"\n-- mvp.toml --\n");
        let mvp_content = std::fs::read(&mvp_path).map_err(|e| {
            WrightError::context(
                format!("failed to read {} for plan fingerprint", mvp_path.display()),
                e,
            )
        })?;
        hasher.update(&mvp_content);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn resolve_explicit_plan_names(
    plan_dirs: &[PathBuf],
    targets: &[String],
) -> Result<HashSet<String>> {
    let index = wright_plan::discovery::PlanIndex::discover(plan_dirs)?;
    let paths = resolve_targets(targets, &index, plan_dirs)?;
    Ok(paths
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| m.metadata.name)
        .collect())
}

/// Outcome of [`resolve_build_set`]: the plan names to build plus, when
/// reverse-dependency expansion ran, why each pulled-in plan is rebuilt and
/// which rebuilt dependency triggered its inclusion.
#[derive(Debug, Clone, Default)]
pub struct ResolvedBuildSet {
    pub names: Vec<String>,
    pub rebuild_reasons: HashMap<String, RebuildReason>,
    pub rebuild_triggers: HashMap<String, String>,
}

pub async fn resolve_build_set(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: ResolveOptions,
) -> Result<ResolvedBuildSet> {
    let plan_dirs = plan_search_dirs(config);
    let index = wright_plan::discovery::PlanIndex::discover(&plan_dirs)?;
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
    let mut rebuild_reasons = HashMap::new();
    let mut rebuild_triggers = HashMap::new();
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
        let db = InstalledDb::open(&db_path, Some(&crate::ledger::dir(config, Some(&db_path))))
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

        if !opts.rdeps.is_empty() {
            // Deployed plans, not parts: reverse-dependency expansion
            // reasons about plan names, and a plan's name need not appear
            // among deployed part names at all (multi-output plans).
            let mut installed_names: HashSet<String> = HashSet::new();
            for plan in db
                .list_plans()
                .await
                .context("failed to list deployed plans for dependents filter")?
            {
                if !db
                    .get_parts_by_plan_id(plan.id)
                    .await
                    .context("failed to list plan outputs")?
                    .is_empty()
                {
                    installed_names.insert(plan.name);
                }
            }
            let expansion = expand_rebuild_deps(
                &mut plans_to_build,
                &index,
                opts.rdeps,
                actual_max,
                &installed_names,
                &config.build.stable_toolchain,
            )
            .await?;
            rebuild_reasons = expansion.reasons;
            rebuild_triggers = expansion.triggers;
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

    Ok(ResolvedBuildSet {
        names,
        rebuild_reasons,
        rebuild_triggers,
    })
}

pub fn create_execution_plan(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: &BuildPlanOptions,
    dep_domain: DepDomain,
) -> Result<BuildExecutionPlan> {
    let plan_dirs = plan_search_dirs(config);
    let index = wright_plan::discovery::PlanIndex::discover(&plan_dirs)?;
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

    pub fn label_for_task(&self, task_name: &str, opts: &BuildPlanOptions) -> &'static str {
        construction_plan_label(task_name, &self.build_set, &self.rebuild_reasons, opts)
    }

    pub fn describe_task(&self, task_name: &str, opts: &BuildPlanOptions) -> String {
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
    crate::util::display::describe_build_capacity(resources.concurrent_tasks, resources.total_cpus)
}

pub fn describe_task_action(task_name: &str, label: &str) -> String {
    let plan_name = BuildExecutionPlan::task_base_name(task_name);
    match label {
        "build" => format!("forge {}", plan_name),
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
    opts: &BuildPlanOptions,
) -> String {
    let mut actions = Vec::with_capacity(tasks.len());
    for task in tasks {
        actions.push(plan.describe_task(task, opts));
    }
    actions.join(", ")
}

pub fn lint_dependency_graph_for_targets(config: &GlobalConfig, targets: &[String]) -> Result<()> {
    let plan_dirs = plan_search_dirs(config);
    let index = wright_plan::discovery::PlanIndex::discover(&plan_dirs)?;
    let plans_to_build = resolve_targets(targets, &index, &plan_dirs)?;

    if plans_to_build.is_empty() {
        return Ok(());
    }

    lint_static_plan_diagnostics(&plans_to_build);

    // Cross-plan naming collisions and dependency-reference integrity
    // (docs/reference/plan-manifest.md promises these checks). Warnings are
    // advisory; a `plan:output` reference naming an output the plan does
    // not declare is an error.
    let mut ref_warnings = Vec::new();
    let mut ref_errors = Vec::new();
    for path in &plans_to_build {
        if let Ok(manifest) = wright_plan::PlanManifest::from_file(path) {
            dep_reference_diagnostics(
                &manifest.metadata.name,
                &manifest,
                &index,
                &mut ref_warnings,
                &mut ref_errors,
            );
        }
    }
    let mut warnings = namespace_collision_warnings(&index);
    warnings.extend(ref_warnings);
    warnings.sort();
    warnings.dedup();
    for warning in &warnings {
        crate::outln!("  [warning] {}", warning);
    }
    ref_errors.sort();
    ref_errors.dedup();
    for error in &ref_errors {
        crate::outln!("  [error] {}", error);
    }
    if !ref_errors.is_empty() {
        return Err(WrightError::ValidationError(format!(
            "lint found {} dependency reference error(s)",
            ref_errors.len()
        )));
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

/// Output names declared by a manifest: the `[[output]]` names in
/// multi-output mode, or the plan name itself for single-output plans.
/// (Local copy of `operations::install::manifest_part_names`; kept here so
/// the resolve layer does not depend on the operations layer.)
fn manifest_output_names(manifest: &PlanManifest) -> Vec<String> {
    match manifest.outputs {
        Some(wright_plan::manifest::OutputConfig::Multi(ref parts)) => {
            parts.iter().map(|(n, _)| n.clone()).collect()
        }
        _ => vec![manifest.metadata.name.clone()],
    }
}

/// Check one plan's dependency references against the plan index.
///
/// - A reference to a plan missing from the index is a warning: it may be
///   satisfied by an external `[[provide]]` at deploy time, but a typo
///   looks exactly the same, so it is reported.
/// - A `plan:output` reference whose output the plan does not declare is
///   an error: unambiguously broken.
fn dep_reference_diagnostics(
    name: &str,
    manifest: &PlanManifest,
    index: &wright_plan::discovery::PlanIndex,
    warnings: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    let dep_fields = [
        ("build_deps", &manifest.build_deps),
        ("link_deps", &manifest.link_deps),
        ("runtime_deps", &manifest.runtime_deps),
    ];

    for (field, deps) in dep_fields {
        for dep_raw in deps {
            let dep_name = version::parse_dependency(dep_raw)
                .unwrap_or_else(|_| (dep_raw.clone(), None))
                .0;
            let dep_ref = version::parse_dep_ref(&dep_name);
            let plan_name = dep_ref.plan();

            let Some(dep_path) = index.path_for(plan_name) else {
                warnings.push(format!(
                    "plan '{}' {}: referenced plan '{}' is not in the plan index \
                     (only acceptable if externally provided at deploy time)",
                    name, field, plan_name
                ));
                continue;
            };

            if let Some(output) = dep_ref.output() {
                let declares = wright_plan::PlanManifest::from_file(dep_path)
                    .map(|m| manifest_output_names(&m))
                    .unwrap_or_default();
                if !declares.is_empty() && !declares.iter().any(|n| n == output) {
                    errors.push(format!(
                        "plan '{}' {}: plan '{}' declares no output named '{}' (declares: {})",
                        name,
                        field,
                        plan_name,
                        output,
                        declares.join(", ")
                    ));
                }
            }
        }
    }
}

/// Warn about cross-plan name collisions in the index: an output shadowing
/// another plan's name, or one output name declared by several plans. Both
/// are tolerated at deploy time (the universal identifier disambiguates
/// user targets), but they are flagged so plan authors opt in deliberately.
fn namespace_collision_warnings(index: &wright_plan::discovery::PlanIndex) -> Vec<String> {
    let Ok(all_manifests) = index.load_all() else {
        return Vec::new();
    };

    let mut plan_names = std::collections::HashSet::new();
    let mut output_owners: HashMap<String, Vec<String>> = HashMap::new();
    for (plan_name, manifest) in &all_manifests {
        plan_names.insert(plan_name.clone());
        for output in manifest_output_names(manifest) {
            output_owners
                .entry(output)
                .or_default()
                .push(plan_name.clone());
        }
    }

    let mut warnings = Vec::new();
    for (output, mut owners) in output_owners {
        owners.sort();
        owners.dedup();
        if owners.len() > 1 {
            warnings.push(format!(
                "output name '{}' is declared by plans {}; deployed part names are globally \
                 unique, so these outputs can never coexist on one system",
                output,
                owners.join(", ")
            ));
        }
        if plan_names.contains(&output)
            && let Some(foreign) = owners.iter().find(|o| *o != &output)
        {
            warnings.push(format!(
                "output '{}' of plan '{}' shadows the plan named '{}'; bare references \
                 resolve to the plan — write '{}:{}' where the output is meant",
                output, foreign, output, foreign, output
            ));
        }
    }
    warnings
}

fn lint_static_plan_diagnostics(plans: &std::collections::HashSet<std::path::PathBuf>) {
    let mut total_warnings = 0;
    for path in plans {
        if let Ok(manifest) = wright_plan::PlanManifest::from_file(path) {
            let diagnostics = wright_plan::lint_manifest(&manifest);
            if !diagnostics.is_empty() {
                crate::outln!("\nPlan Style & Best Practice Report: {}", path.display());
                for diag in &diagnostics {
                    total_warnings += 1;
                    crate::outln!("  [{}] {}: {}", diag.code, diag.level, diag.message);
                    if let Some(ref help) = diag.help {
                        crate::outln!("     └─ help: {}", help);
                    }
                }
            }
        }
    }
    if total_warnings > 0 {
        crate::outln!(
            "\nTotal style diagnostics: {} warning(s) found.",
            total_warnings
        );
    }
}

fn lint_dependency_graph(graph: &bootstrap::PlanGraph) -> Result<()> {
    use bootstrap::{cycle_candidates_for, find_cycles, format_cycle_path, pick_candidate};
    let cycles = find_cycles(&graph.deps_map);

    crate::outln!("Dependency Analysis Report");
    crate::outln!(
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

    crate::outln!();
    crate::outln!("Cycles ({}):", cycles.len());
    for (idx, cycle) in cycles.iter().enumerate() {
        crate::outln!("{}: {}", idx + 1, format_cycle_path(cycle, &graph.deps_map));
    }

    crate::outln!();
    crate::outln!("MVP Candidates (deterministic pick = fewest excluded edges, then name):");
    crate::outln!("Cycle | Candidate | Excludes | Selected");
    crate::outln!("----- | --------- | -------- | --------");
    for (idx, cycle) in cycles.iter().enumerate() {
        let candidates = cycle_candidates_for(cycle, graph);
        if candidates.is_empty() {
            crate::outln!("{} | - | - | no candidates", idx + 1);
            continue;
        }
        let chosen = pick_candidate(candidates.clone());
        for cand in candidates {
            let selected = match &chosen {
                Some(c) if c.part == cand.part && c.excluded == cand.excluded => "yes",
                _ => "no",
            };
            crate::outln!(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plan(plans_dir: &Path, name: &str, extra: &str) -> PathBuf {
        let plan_dir = plans_dir.join(name);
        std::fs::create_dir_all(&plan_dir).unwrap();
        let path = plan_dir.join("plan.toml");
        std::fs::write(
            &path,
            format!(
                "name = \"{name}\"\nversion = \"1.0.0\"\nrelease = 1\ndescription = \"d\"\nlicense = \"MIT\"\narch = \"x86_64\"\n{extra}"
            ),
        )
        .unwrap();
        path
    }

    fn lint_index(plans_dir: &Path) -> wright_plan::discovery::PlanIndex {
        wright_plan::discovery::PlanIndex::discover(&[plans_dir.to_path_buf()]).unwrap()
    }

    #[test]
    fn dep_reference_diagnostics_flags_missing_and_undeclared() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        write_plan(
            &plans_dir,
            "a",
            "[[output]]\nname = \"x\"\ndescription = \"x\"\ninclude = [\"/usr/lib/**\"]\n",
        );
        let c_path = write_plan(&plans_dir, "c", "");
        // c: build dep on a missing plan (warning), link dep on an
        // undeclared output (error), runtime deps on a declared output and
        // a constrained in-index plan (both clean).
        std::fs::write(
            &c_path,
            "name = \"c\"\nversion = \"1.0.0\"\nrelease = 1\ndescription = \"d\"\nlicense = \"MIT\"\narch = \"x86_64\"\nbuild_deps = [\"nope\"]\nlink_deps = [\"a:y\"]\nruntime_deps = [\"a:x\", \"a >= 1.0\"]\n",
        )
        .unwrap();

        let index = lint_index(&plans_dir);
        let manifest = PlanManifest::from_file(&c_path).unwrap();
        let (mut warnings, mut errors) = (Vec::new(), Vec::new());
        dep_reference_diagnostics("c", &manifest, &index, &mut warnings, &mut errors);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("nope"), "{:?}", warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("no output named 'y'"), "{:?}", errors);
    }

    #[test]
    fn namespace_collision_warnings_cover_shadowing_and_coexistence() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        write_plan(
            &plans_dir,
            "a",
            "[[output]]\nname = \"x\"\ndescription = \"x\"\ninclude = [\"/usr/lib/**\"]\n",
        );
        write_plan(&plans_dir, "x", "");

        let warnings = namespace_collision_warnings(&lint_index(&plans_dir));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("shadows the plan named 'x'")),
            "shadowing warning present: {:?}",
            warnings
        );
        assert!(
            warnings.iter().any(|w| w.contains("declared by plans")),
            "coexistence warning present: {:?}",
            warnings
        );
    }

    #[test]
    fn namespace_collision_warnings_ignore_coinciding_single_output() {
        let temp = tempfile::tempdir().unwrap();
        let plans_dir = temp.path().join("plans");
        write_plan(&plans_dir, "zlib", "");

        let warnings = namespace_collision_warnings(&lint_index(&plans_dir));
        assert!(
            warnings.is_empty(),
            "no warnings for the coincide case: {:?}",
            warnings
        );
    }
}
