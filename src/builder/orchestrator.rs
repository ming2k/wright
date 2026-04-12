//! Build orchestrator — parallel build scheduling, dependency resolution,
//! cascade expansion, and automatic bootstrap cycle detection/resolution.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::builder::logging;
use crate::builder::mvp::inject_bootstrap_passes;
use crate::error::{Result, WrightError, WrightResultExt};
use tracing::{info, warn};

use crate::config::GlobalConfig;
use crate::database::Database;
use crate::plan::manifest::PlanManifest;

mod execute;
mod planning;
mod resolver;

use execute::{execute_builds, lint_dependency_graph};
#[cfg(test)]
use planning::construction_plan_order;
#[cfg(test)]
use planning::installed_matches_manifest;
use planning::{
    build_dep_map, compute_session_hash, construction_plan_batches, construction_plan_label,
    expand_missing_dependencies, expand_rebuild_deps,
};
use resolver::resolve_targets;

pub use resolver::setup_resolver;

use crate::inventory::resolver::LocalResolver;

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
    /// Include the listed targets themselves in the output.
    pub include_targets: bool,
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
    /// Resume a previous build session. `Some(None)` auto-detects
    /// the session from the build set hash; `Some(Some(hash))` resumes
    /// a specific session.
    pub resume: Option<Option<String>>,
    pub checksum: bool,
    /// Skip the lifecycle `check` stage during a full build.
    /// Ignored for metadata-only operations and when explicit `--stage` selection is used.
    pub skip_check: bool,
    pub verbose: bool,
    pub quiet: bool,
    /// --mvp: build using mvp.toml deps without requiring a cycle to trigger it.
    pub mvp: bool,
    /// Print produced part paths to stdout.
    pub print_parts: bool,
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

/// Resolve targets to their canonical plan names without any dependency expansion.
/// Used by `apply` to determine which targets were explicitly requested by the user
/// (vs. pulled in as sync dependencies), so that install origin can be set correctly.
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

/// Resolve targets and expand their dependency graph according to the given options.
/// Returns a list of plan names suitable for piping into `wright build`.
pub fn resolve_build_set(
    config: &GlobalConfig,
    targets: Vec<String>,
    opts: ResolveOptions,
) -> Result<Vec<String>> {
    let resolver = setup_resolver(config)?;
    let all_plans = resolver.get_all_plans()?;
    let mut plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;

    if plans_to_build.is_empty() {
        return Err(WrightError::BuildError(
            "No targets specified to resolve.".to_string(),
        ));
    }

    let actual_max = {
        let max_depth = opts.depth.unwrap_or(1);
        if max_depth == 0 {
            usize::MAX
        } else {
            max_depth
        }
    };

    let original_plans: HashSet<PathBuf> = plans_to_build.clone();

    {
        let db_path = config.general.db_path.clone();
        let db = Database::open(&db_path)
            .context("failed to open database for dependency resolution")?;

        // 1. Traverse upstream
        if let Some(domain) = opts.deps {
            expand_missing_dependencies(
                &mut plans_to_build,
                &all_plans,
                &db,
                &opts.match_policies,
                domain,
                actual_max,
            )?;
        }

        // 2. Filter the targets and expanded upstream deps
        if !opts.match_policies.contains(&MatchPolicy::All) {
            plans_to_build.retain(|path| {
                if let Ok(m) = PlanManifest::from_file(path) {
                    crate::builder::orchestrator::planning::dependency_matches_policy(
                        &m.plan.name,
                        &all_plans,
                        &db,
                        &opts.match_policies,
                    )
                    .unwrap_or(true)
                } else {
                    true
                }
            });
        }

        // 3. Traverse downstream from the filtered changing set
        if let Some(domain) = opts.rdeps {
            let installed_names: HashSet<String> = db
                .list_parts()
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
            )?;
        }
    }

    if !opts.include_targets {
        plans_to_build.retain(|p| !original_plans.contains(p));
    }

    let names: Vec<String> = plans_to_build
        .iter()
        .filter_map(|p| PlanManifest::from_file(p).ok())
        .map(|m| m.plan.name)
        .collect();

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

    /// Strip the internal `:bootstrap` suffix used for MVP cycle-breaking tasks
    /// to get the canonical plan name for display and explicit-target matching.
    pub fn task_base_name(task: &str) -> &str {
        task.trim_end_matches(":bootstrap")
    }

    pub fn execute_batch(
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
        )
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

/// Run a multi-target build with dependency ordering and parallel execution.
/// Targets are plan names, paths, or @assemblies — no dependency expansion is performed.
/// Use `resolve_build_set` (via `wright resolve`) to expand deps/dependents before calling this.
pub fn run_build(config: &GlobalConfig, targets: Vec<String>, opts: BuildOptions) -> Result<()> {
    if opts.lint {
        let resolver = setup_resolver(config)?;
        let all_plans = resolver.get_all_plans()?;
        let plans_to_build = resolve_targets(&targets, &all_plans, &resolver)?;
        return lint_dependency_graph(&plans_to_build, &all_plans, build_dep_map);
    }

    let plan = create_execution_plan(config, targets, &opts)?;

    // --- Session management for --resume ---
    let session_hash = if opts.resume.is_some() || opts.is_build_op() {
        Some(compute_session_hash(&plan.build_set))
    } else {
        None
    };

    // When resuming, resolve the session hash (auto-detect or explicit).
    let (_effective_resume, session_completed) = match &opts.resume {
        Some(explicit_hash) => {
            let hash = match explicit_hash {
                Some(h) => h.clone(),
                None => session_hash.clone().unwrap_or_default(),
            };
            if hash.is_empty() {
                (false, HashSet::new())
            } else {
                let db = Database::open(&config.general.db_path)
                    .context("failed to open database for resume")?;
                if db.session_exists(&hash)? {
                    let completed = db.get_session_completed(&hash)?;
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

    // --- Build Plan Summary ---
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

    // Create/update session in DB for build operations.
    let active_session_hash = if let Some(ref hash) = session_hash {
        if opts.is_build_op() {
            if let Ok(db) = Database::open(&config.general.db_path) {
                let packages: Vec<String> = plan.name_to_path.keys().cloned().collect();
                let _ = db.create_session(hash, &packages);
            }
            Some(hash.clone())
        } else {
            None
        }
    } else {
        None
    };

    // 3. Execute builds
    let result = execute_builds(
        config,
        &plan.name_to_path,
        &plan.deps_map,
        &plan.build_set,
        &opts,
        &plan.bootstrap_excluded,
        active_session_hash.as_deref(),
        &session_completed,
    );

    match &result {
        Ok(()) => {
            // All done — clean up session
            if let Some(ref hash) = active_session_hash {
                if let Ok(db) = Database::open(&config.general.db_path) {
                    let _ = db.clear_session(hash);
                }
            }
        }
        Err(_) => {
            // Print session hash so user can resume
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

#[cfg(test)]
mod tests {
    use super::{
        construction_plan_batches, construction_plan_label, construction_plan_order,
        describe_build_resources, describe_task_action, expand_missing_dependencies,
        installed_matches_manifest, BuildOptions, BuildResourceSummary, DependentsMode,
        MatchPolicy, RebuildReason,
    };
    use crate::database::{Database, InstalledPart, NewPart};
    use crate::plan::manifest::PlanManifest;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::PathBuf;

    fn write_plan(
        dir: &std::path::Path,
        name: &str,
        version: &str,
        build_deps: &[&str],
    ) -> PathBuf {
        let plan_dir = dir.join(name);
        fs::create_dir_all(&plan_dir).unwrap();
        let build = if build_deps.is_empty() {
            String::new()
        } else {
            format!(
                "\n[dependencies]\nbuild = [{}]\n",
                build_deps
                    .iter()
                    .map(|dep| format!("\"{}\"", dep))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let content = format!(
            r#"name = "{name}"
version = "{version}"
release = 1
description = "test part"
license = "MIT"
arch = "x86_64"{build}
[lifecycle.staging]
executor = "shell"
dockyard = "none"
script = "mkdir -p ${{PART_DIR}}/usr/bin"
"#
        );
        let path = plan_dir.join("plan.toml");
        fs::write(&path, content).unwrap();
        path
    }

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
            origin: crate::database::Origin::Manual,
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

    #[test]
    fn sync_traversal_reaches_outdated_descendant_through_up_to_date_parent() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("parts.db");
        let db = Database::open(&db_path).unwrap();
        db.insert_part(NewPart {
            name: "b",
            version: "1.0.0",
            release: 1,
            epoch: 0,
            description: "b",
            arch: "x86_64",
            license: "MIT",
            url: None,
            install_size: 1,
            pkg_hash: None,
            install_scripts: None,
            origin: crate::database::Origin::Manual,
        })
        .unwrap();
        db.insert_part(NewPart {
            name: "c",
            version: "0.9.0",
            release: 1,
            epoch: 0,
            description: "c",
            arch: "x86_64",
            license: "MIT",
            url: None,
            install_size: 1,
            pkg_hash: None,
            install_scripts: None,
            origin: crate::database::Origin::Manual,
        })
        .unwrap();

        let plans_root = temp.path().join("plans");
        let a_path = write_plan(&plans_root, "a", "1.0.0", &["b"]);
        let b_path = write_plan(&plans_root, "b", "1.0.0", &["c"]);
        let c_path = write_plan(&plans_root, "c", "1.0.0", &[]);

        let mut plans_to_build = std::collections::HashSet::from([a_path.clone()]);
        let all_plans = HashMap::from([
            ("a".to_string(), a_path),
            ("b".to_string(), b_path),
            ("c".to_string(), c_path.clone()),
        ]);

        expand_missing_dependencies(
            &mut plans_to_build,
            &all_plans,
            &db,
            &[MatchPolicy::Outdated],
            DependentsMode::All,
            usize::MAX,
        )
        .unwrap();

        assert!(!plans_to_build.contains(&all_plans["b"]));
        assert!(plans_to_build.contains(&c_path));
    }

    #[test]
    fn construction_plan_uses_stable_topological_order() {
        let build_set = HashSet::from([
            "pcre2".to_string(),
            "librsvg:bootstrap".to_string(),
            "gdk-pixbuf".to_string(),
            "librsvg".to_string(),
        ]);
        let deps_map = HashMap::from([
            ("pcre2".to_string(), Vec::new()),
            ("librsvg:bootstrap".to_string(), Vec::new()),
            (
                "gdk-pixbuf".to_string(),
                vec!["librsvg:bootstrap".to_string()],
            ),
            (
                "librsvg".to_string(),
                vec!["gdk-pixbuf".to_string(), "librsvg:bootstrap".to_string()],
            ),
        ]);

        let ordered = construction_plan_order(&build_set, &deps_map);

        assert_eq!(
            ordered,
            vec![
                ("librsvg:bootstrap".to_string(), 0),
                ("pcre2".to_string(), 0),
                ("gdk-pixbuf".to_string(), 1),
                ("librsvg".to_string(), 2),
            ]
        );
    }

    #[test]
    fn construction_plan_batches_use_dependency_waves() {
        let build_set = HashSet::from([
            "systemd".to_string(),
            "libusb".to_string(),
            "procps-ng".to_string(),
            "podman".to_string(),
        ]);
        let deps_map = HashMap::from([
            ("systemd".to_string(), vec![]),
            ("libusb".to_string(), vec!["systemd".to_string()]),
            ("procps-ng".to_string(), vec!["systemd".to_string()]),
            (
                "podman".to_string(),
                vec!["libusb".to_string(), "procps-ng".to_string()],
            ),
        ]);

        let ordered = construction_plan_batches(&build_set, &deps_map);
        assert_eq!(
            ordered,
            vec![
                ("systemd".to_string(), 0),
                ("libusb".to_string(), 1),
                ("procps-ng".to_string(), 1),
                ("podman".to_string(), 2),
            ]
        );
    }

    #[test]
    fn construction_plan_labels_use_action_semantics() {
        let build_set = HashSet::from(["librsvg:bootstrap".to_string(), "librsvg".to_string()]);
        let reasons = HashMap::from([
            ("pcre2".to_string(), RebuildReason::Explicit),
            ("gtk4".to_string(), RebuildReason::LinkDependency),
            ("vala".to_string(), RebuildReason::Transitive),
        ]);
        let opts = BuildOptions::default();
        let mvp_opts = BuildOptions {
            mvp: true,
            ..BuildOptions::default()
        };

        assert_eq!(
            construction_plan_label("librsvg:bootstrap", &build_set, &reasons, &opts),
            "build:mvp"
        );
        assert_eq!(
            construction_plan_label("librsvg", &build_set, &reasons, &opts),
            "build:full"
        );
        assert_eq!(
            construction_plan_label("pcre2", &HashSet::new(), &reasons, &opts),
            "build"
        );
        assert_eq!(
            construction_plan_label("gtk4", &HashSet::new(), &reasons, &opts),
            "relink"
        );
        assert_eq!(
            construction_plan_label("vala", &HashSet::new(), &reasons, &opts),
            "rebuild"
        );
        assert_eq!(
            construction_plan_label("freetype", &HashSet::new(), &HashMap::new(), &mvp_opts),
            "build:mvp"
        );

        assert_eq!(
            describe_task_action("librsvg:bootstrap", "build:mvp"),
            "bootstrap librsvg"
        );
        assert_eq!(
            describe_task_action("librsvg", "build:full"),
            "full rebuild librsvg"
        );
        assert_eq!(describe_task_action("pcre2", "build"), "build pcre2");
        assert_eq!(describe_task_action("gtk4", "relink"), "relink gtk4");
        assert_eq!(describe_task_action("vala", "rebuild"), "rebuild vala");
    }

    #[test]
    fn build_resource_summary_messages_are_human_readable() {
        assert_eq!(
            describe_build_resources(BuildResourceSummary {
                total_cpus: 14,
                concurrent_tasks: 14,
            }),
            "Build capacity: 14 parallel tasks on 14 CPU cores."
        );
    }
}
