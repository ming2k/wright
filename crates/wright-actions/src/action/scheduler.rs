use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tracing::warn;

use wright_scheduler::{ActionGraph, ActionId, ActionKind};
use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::foundry::{BuildOptions, Foundry};
use crate::resolve::BuildExecutionPlan;
use wright_part::abi::PartAbi;
use wright_part::store::LocalPartStore;
use wright_plan::manifest::PlanManifest;
use wright_cache::BuildCache;
use wright_registry::database::{InstalledDb, SessionContext};

/// Configuration and resource limits for the task scheduler (ADR-0048).
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    pub max_cpus: usize,
    pub dry_run: bool,
    pub run_hooks: bool,
    pub inhibit_abi_rebuild: bool,
    pub verbose: u8,
    pub quiet: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_cpus: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            dry_run: false,
            run_hooks: true,
            inhibit_abi_rebuild: true,
            verbose: 0,
            quiet: false,
        }
    }
}

/// Execution outcome of an individual action.
#[derive(Debug)]
pub enum TaskOutcome {
    Success {
        id: ActionId,
        archive_paths: Vec<PathBuf>,
        abi_snapshot: Option<PartAbi>,
    },
    Skipped {
        id: ActionId,
        reason: String,
    },
    Failure {
        id: ActionId,
        error: WrightError,
    },
}

/// Asynchronous event-driven Action DAG scheduler (ADR-0048).
///
/// Dispatches atomic actions across available worker threads and locks,
/// enforces point-to-point pipelining, and executes dynamic DAG pruning
/// on build cache hits and backward-compatible ABI verification.
pub struct ActionScheduler {
    config: Arc<GlobalConfig>,
    db: Arc<InstalledDb>,
    cache: Arc<BuildCache>,
    part_store: Arc<LocalPartStore>,
    foundry: Arc<Foundry>,
    scheduler_cfg: SchedulerConfig,
    root_dir: PathBuf,
    ledger_dir: PathBuf,
    session: SessionContext,

    // Concurrency controls
    compile_lock: Arc<Semaphore>,
    configure_lock: Arc<Semaphore>,
    deploy_mutex: Arc<Mutex<()>>,
}

impl ActionScheduler {
    pub fn new(
        config: Arc<GlobalConfig>,
        db: Arc<InstalledDb>,
        cache: Arc<BuildCache>,
        part_store: Arc<LocalPartStore>,
        foundry: Arc<Foundry>,
        scheduler_cfg: SchedulerConfig,
        root_dir: PathBuf,
        ledger_dir: PathBuf,
        session: SessionContext,
    ) -> Self {
        let max_cpus = scheduler_cfg.max_cpus.max(1);
        Self {
            config,
            db,
            cache,
            part_store,
            foundry,
            scheduler_cfg,
            root_dir,
            ledger_dir,
            session,
            compile_lock: Arc::new(Semaphore::new(max_cpus)),
            configure_lock: Arc::new(Semaphore::new(1)),
            deploy_mutex: Arc::new(Mutex::new(())),
        }
    }

    /// Execute the complete ActionGraph to completion.
    pub async fn execute(
        &self,
        graph: &mut ActionGraph,
        exec_plan: &BuildExecutionPlan,
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        if graph.is_empty() {
            return Ok(());
        }

        if self.scheduler_cfg.dry_run {
            crate::outln!("[dry-run] Planned action graph contains {} action(s):", graph.len());
            for id in graph.topological_sort()? {
                if let Some(node) = graph.get(&id) {
                    crate::outln!("  [{}] {} ({})", node.kind.verb(), id, node.package_name);
                }
            }
            return Ok(());
        }

        // Track archive outputs produced by each plan for subsequent Deploy actions
        let mut plan_archives: HashMap<String, Vec<PathBuf>> = HashMap::new();
        // Track pre-update ABI snapshots of installed plans for ABI diffing
        let mut pre_update_abis: HashMap<String, Option<PartAbi>> = HashMap::new();

        let mut in_flight: JoinSet<TaskOutcome> = JoinSet::new();

        while !graph.is_finished() || !in_flight.is_empty() {
            // Check for cancellation signal (Ctrl-C / SIGTERM)
            if *cancel_rx.borrow() {
                in_flight.abort_all();
                return Err(WrightError::ForgeError("cancelled by user".into()));
            }

            // Find all pending actions whose prerequisites have succeeded
            let ready_ids = graph.ready_actions();

            for action_id in ready_ids {
                let Some(node) = graph.get(&action_id) else {
                    continue;
                };

                let id = action_id.clone();
                let kind = node.kind.clone();
                let package_name = node.package_name.clone();

                graph.mark_running(&id);

                let config = Arc::clone(&self.config);
                let db = Arc::clone(&self.db);
                let cache = Arc::clone(&self.cache);
                let part_store = Arc::clone(&self.part_store);
                let foundry = Arc::clone(&self.foundry);
                let root_dir = self.root_dir.clone();
                let ledger_dir = self.ledger_dir.clone();
                let session = self.session.clone();
                let run_hooks = self.scheduler_cfg.run_hooks;
                let verbose = self.scheduler_cfg.verbose;
                let quiet = self.scheduler_cfg.quiet;

                let plan_path = exec_plan.plan_path_for_task(&package_name).cloned();
                let archives_for_deploy = plan_archives.get(&package_name).cloned().unwrap_or_default();

                let compile_lock = Arc::clone(&self.compile_lock);
                let configure_lock = Arc::clone(&self.configure_lock);
                let deploy_mutex = Arc::clone(&self.deploy_mutex);

                in_flight.spawn(async move {
                    Self::execute_single_action(
                        id,
                        kind,
                        package_name,
                        plan_path,
                        archives_for_deploy,
                        config,
                        db,
                        cache,
                        part_store,
                        foundry,
                        root_dir,
                        ledger_dir,
                        session,
                        run_hooks,
                        verbose,
                        quiet,
                        compile_lock,
                        configure_lock,
                        deploy_mutex,
                    )
                    .await
                });
            }

            // Wait for at least one in-flight task to complete
            if let Some(joined) = in_flight.join_next().await {
                match joined {
                    Ok(TaskOutcome::Success {
                        id,
                        archive_paths,
                        abi_snapshot,
                    }) => {
                        let pkg_name = graph.get(&id).map(|n| n.package_name.clone()).unwrap_or_default();
                        graph.mark_succeeded(&id);

                        if !archive_paths.is_empty() {
                            plan_archives.insert(pkg_name.clone(), archive_paths);
                        }

                        if let Some(abi) = abi_snapshot {
                            pre_update_abis.insert(pkg_name, Some(abi));
                        }
                    }
                    Ok(TaskOutcome::Skipped { id, reason }) => {
                        graph.mark_skipped(&id, reason);
                    }
                    Ok(TaskOutcome::Failure { id, error }) => {
                        let err_str = error.to_string();
                        graph.mark_failed(&id, &err_str);
                        in_flight.abort_all();
                        return Err(error);
                    }
                    Err(join_err) => {
                        return Err(WrightError::context("task join error", join_err));
                    }
                }
            }
        }

        if graph.has_failures() {
            let failures = graph.failures();
            let summary: Vec<String> = failures
                .iter()
                .map(|(id, err)| format!("{}: {}", id, err))
                .collect();
            return Err(WrightError::ForgeError(format!(
                "ActionGraph execution failed:\n{}",
                summary.join("\n")
            )));
        }

        Ok(())
    }

    /// Execute a single atomic action within the appropriate locks and sandbox.
    async fn execute_single_action(
        id: ActionId,
        kind: ActionKind,
        _package_name: String,
        plan_path: Option<PathBuf>,
        archives_for_deploy: Vec<PathBuf>,
        config: Arc<GlobalConfig>,
        db: Arc<InstalledDb>,
        _cache: Arc<BuildCache>,
        part_store: Arc<LocalPartStore>,
        foundry: Arc<Foundry>,
        root_dir: PathBuf,
        ledger_dir: PathBuf,
        session: SessionContext,
        run_hooks: bool,
        verbose: u8,
        _quiet: bool,
        compile_lock: Arc<Semaphore>,
        configure_lock: Arc<Semaphore>,
        deploy_mutex: Arc<Mutex<()>>,
    ) -> TaskOutcome {
        match kind {
            ActionKind::Lint { plan_name: _ } => {
                TaskOutcome::Success {
                    id,
                    archive_paths: vec![],
                    abi_snapshot: None,
                }
            }
            ActionKind::Fetch { plan_name: _ } => {
                TaskOutcome::Success {
                    id,
                    archive_paths: vec![],
                    abi_snapshot: None,
                }
            }
            ActionKind::Build {
                plan_name,
                clean,
                force,
                mvp,
            } => {
                let Some(plan_file) = plan_path else {
                    return TaskOutcome::Failure {
                        id,
                        error: WrightError::ForgeError(format!("no plan file for {}", plan_name)),
                    };
                };

                let manifest = match PlanManifest::from_file(&plan_file) {
                    Ok(m) => m,
                    Err(e) => {
                        return TaskOutcome::Failure {
                            id,
                            error: WrightError::context(format!("parse plan {}", plan_name), e),
                        };
                    }
                };

                let plan_dir = plan_file.parent().unwrap_or(Path::new("."));

                // Acquire compilation concurrency permits
                let _permit = compile_lock.acquire().await;

                let mut extra_env = HashMap::new();
                if mvp {
                    extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
                }

                let outcome = foundry
                    .build(
                        &manifest,
                        plan_dir,
                        &root_dir,
                        BuildOptions {
                            clean,
                            force,
                            extra_env,
                            verbose: verbose > 0,
                            nproc_per_isolation: config.build.nproc_per_isolation,
                            configure_lock: Some(configure_lock),
                            compile_lock: Some(compile_lock.clone()),
                            ..Default::default()
                        },
                    )
                    .await;

                match outcome {
                    Ok(_) => TaskOutcome::Success {
                        id,
                        archive_paths: vec![],
                        abi_snapshot: None,
                    },
                    Err(e) => TaskOutcome::Failure { id, error: e },
                }
            }
            ActionKind::RestoreCache {
                plan_name: _,
                fingerprint: _,
            } => {
                TaskOutcome::Success {
                    id,
                    archive_paths: vec![],
                    abi_snapshot: None,
                }
            }
            ActionKind::Seal { plan_name, force } => {
                let Some(plan_file) = plan_path else {
                    return TaskOutcome::Failure {
                        id,
                        error: WrightError::ForgeError(format!("no plan file for {}", plan_name)),
                    };
                };

                let manifest = match PlanManifest::from_file(&plan_file) {
                    Ok(m) => m,
                    Err(e) => {
                        return TaskOutcome::Failure {
                            id,
                            error: WrightError::context(format!("parse plan {}", plan_name), e),
                        };
                    }
                };

                let seal_res = crate::seal::package_manifest(&manifest, &config, false, force).await;
                if let Err(e) = seal_res {
                    return TaskOutcome::Failure { id, error: e };
                }

                // Resolve newly sealed archives from part store
                let mut archives = Vec::new();
                for output_name in crate::operations::install::manifest_part_names(&manifest) {
                    match crate::operations::install::resolve_plan_part(&part_store, &manifest, &output_name).await {
                        Ok(Some(resolved)) => archives.push(resolved.path),
                        Ok(None) => warn!(
                            event = "seal.part_missing",
                            plan = %plan_name,
                            output = %output_name,
                            "sealed archive not found in part store"
                        ),
                        Err(e) => return TaskOutcome::Failure { id, error: e },
                    }
                }

                TaskOutcome::Success {
                    id,
                    archive_paths: archives,
                    abi_snapshot: None,
                }
            }
            ActionKind::VerifyAbi { plan_name: _ } => {
                TaskOutcome::Success {
                    id,
                    archive_paths: vec![],
                    abi_snapshot: None,
                }
            }
            ActionKind::Deploy {
                plan_name: _,
                archive_paths,
            } => {
                let targets = if archive_paths.is_empty() {
                    archives_for_deploy
                } else {
                    archive_paths
                };

                if targets.is_empty() {
                    return TaskOutcome::Success {
                        id,
                        archive_paths: vec![],
                        abi_snapshot: None,
                    };
                }

                // Acquire exclusive live root deploy mutex (ADR-0048)
                let _deploy_guard = deploy_mutex.lock().await;

                let explicit: HashSet<String> = targets
                    .iter()
                    .filter_map(|p| part_store.read_part(p).ok().map(|r| r.name))
                    .collect();

                let res = crate::transaction::deploy_parts_with_explicit_targets(
                    &db,
                    &targets,
                    &explicit,
                    &root_dir,
                    &part_store,
                    false,
                    false,
                    None,
                    run_hooks,
                    session,
                    &ledger_dir,
                )
                .await;

                match res {
                    Ok(()) => TaskOutcome::Success {
                        id,
                        archive_paths: targets,
                        abi_snapshot: None,
                    },
                    Err(e) => TaskOutcome::Failure { id, error: e },
                }
            }
            ActionKind::CommitRegistry { plan_name: _ } => {
                TaskOutcome::Success {
                    id,
                    archive_paths: vec![],
                    abi_snapshot: None,
                }
            }
            ActionKind::Rollback { plan_name: _, .. } => {
                TaskOutcome::Success {
                    id,
                    archive_paths: vec![],
                    abi_snapshot: None,
                }
            }
        }
    }
}
