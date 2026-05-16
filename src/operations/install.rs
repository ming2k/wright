use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Result, WrightError};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing::{debug, info, trace, warn};

use crate::config::GlobalConfig;
use crate::database::{InstalledDb, SessionContext};
use crate::delivery::store::CasStore;
use crate::foundry::{BuildOptions, Foundry};
use crate::part::folio;
use crate::part::store::LocalPartStore;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::resolve::{
    self, BuildExecutionPlan, BuildPlanOptions, DepDomain, MatchPolicy, ResolveOptions,
    create_execution_plan, resolve_build_set, resolve_explicit_plan_names,
};

pub struct InstallRequest<'a> {
    pub targets: Vec<String>,
    pub dep_domain: DepDomain,
    pub match_policies: Vec<MatchPolicy>,
    pub depth: Option<usize>,
    pub force: bool,
    pub config: &'a GlobalConfig,
    pub db_path: &'a Path,
    pub root_dir: &'a Path,
    pub verbose: u8,
    pub quiet: bool,
    pub part_store: &'a LocalPartStore,
    /// Optional forge options. When provided, the install flow uses these
    /// instead of default BuildPlanOptions (used by `wright build`).
    pub build_opts: Option<BuildPlanOptions>,
}

/// Pre-computed fingerprint for each plan name in the build set.
///
/// The fingerprint captures the plan's own build key and the fingerprints of
/// its direct build dependencies, forming a content-addressed identity that
/// covers the full transitive build closure.
struct PlanFingerprints {
    /// plan_name -> closure fingerprint
    fingerprints: HashMap<String, String>,
}

impl PlanFingerprints {
    /// Compute fingerprints for all plans in the execution plan.
    fn compute(plan: &BuildExecutionPlan, foundry: &Foundry) -> Result<Self> {
        let mut fingerprints: HashMap<String, String> = HashMap::new();

        // Process batch by batch so that dependency fingerprints are available
        // when computing closure fingerprints for later batches.
        for batch in plan.batches() {
            for task in batch {
                let base = BuildExecutionPlan::task_base_name(task);
                let plan_path = plan
                    .plan_path_for_task(task)
                    .ok_or_else(|| WrightError::ForgeError(format!("no path for task {}", task)))?;
                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", base, e)))?;

                let build_key = foundry.compute_build_key(&manifest)?;

                // Collect fingerprints of build dependencies.
                let dep_names = plan.deps_for_task(task);
                let mut dep_fps: HashMap<String, String> = HashMap::new();
                for dep_name in dep_names {
                    let dep_base = BuildExecutionPlan::task_base_name(dep_name);
                    if let Some(fp) = fingerprints.get(dep_base) {
                        dep_fps.insert(dep_base.to_string(), fp.clone());
                    }
                }

                let closure_fp = CasStore::compute_closure_fingerprint(&build_key, &dep_fps);
                trace!(event = "fingerprint.closure", plan_name = %base, closure_fp = %&closure_fp[..8], "Computed closure fingerprint");

                // Insert for both the full task and its bootstrap variant.
                // Bootstrap tasks get a different fingerprint to distinguish
                // from full builds (different compilation results).
                fingerprints.insert(base.to_string(), closure_fp.clone());
                if task.ends_with(":bootstrap") {
                    fingerprints.insert(task.clone(), closure_fp);
                } else {
                    // Also insert the :bootstrap variant if it exists.
                    let bootstrap_task = format!("{}:bootstrap", base);
                    if plan.build_set().contains(&bootstrap_task) {
                        let mut bp = Sha256::new();
                        bp.update(closure_fp.as_bytes());
                        bp.update(b":bootstrap");
                        let bootstrap_fp = format!("{:x}", bp.finalize());
                        trace!(event = "fingerprint.bootstrap", plan_name = %base, bootstrap_fp = %&bootstrap_fp[..8], "Computed bootstrap fingerprint");
                        fingerprints.insert(bootstrap_task, bootstrap_fp);
                    }
                }
            }
        }

        Ok(Self { fingerprints })
    }

    fn get(&self, name: &str) -> Option<&String> {
        self.fingerprints.get(name)
    }
}

pub async fn execute_install(request: InstallRequest<'_>) -> Result<()> {
    let workflow_t0 = std::time::Instant::now();
    let InstallRequest {
        targets,
        dep_domain,
        match_policies,
        depth,
        force,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
        build_opts,
    } = request;

    if targets.is_empty() {
        return Err(WrightError::ForgeError(
            "no targets specified (pass plan names, folio names prefixed with '@', or paths as arguments or via stdin)".into()
        ));
    }

    let folios_dirs: Vec<PathBuf> = vec![config.general.folios_dir.clone()];
    let (targets, folio_assumes, _folio_config) =
        folio::expand_folio_references(targets, &folios_dirs)?;

    if targets.is_empty() {
        return Err(WrightError::ForgeError(
            "no plans to forge after expanding folios".into(),
        ));
    }

    register_folio_assumptions(db_path, &folio_assumes).await?;

    let resolve_opts = ResolveOptions {
        deps: dep_domain,
        rdeps: DepDomain::empty(),
        match_policies: if match_policies.is_empty() {
            vec![MatchPolicy::Outdated]
        } else {
            match_policies
        },
        depth: Some(depth.unwrap_or(0)),
        include_targets: true,
        preserve_targets: force,
    };

    let build_opts = build_opts.unwrap_or_else(|| BuildPlanOptions {
        clean: force,
        force,
        verbose: verbose > 0,
        quiet,
        nproc_per_isolation: config.build.nproc_per_isolation,
        ..Default::default()
    });

    let build_set: Vec<String> = resolve_build_set(config, targets.clone(), resolve_opts.clone())
        .await
        .map_err(|e| WrightError::ForgeError(format!("resolve_build_set: {}", e)))?;

    if build_set.is_empty() {
        if !quiet {
            println!(
                "{} already installed and up to date (use --force to reinstall)",
                targets.join(", ")
            );
        }
        return Ok(());
    }

    let plan_dirs = resolve::plan_search_dirs(config);
    let explicit_plan_names = resolve_explicit_plan_names(&plan_dirs, &targets)
        .map_err(|e| WrightError::ForgeError(format!("explicit plan names: {}", e)))?;

    let plan = create_execution_plan(config, build_set, &build_opts, dep_domain)
        .map_err(|e| WrightError::ForgeError(format!("create_execution_plan: {}", e)))?;

    let total_packages = plan.build_set().len();
    let total_batches = plan.batches().len();
    // Pre-render every batch's task list once; we reuse it for the planning
    // summary (one line per batch when there are >1) and the per-batch
    // structured log entries.
    let batch_entries: Vec<Vec<String>> = plan
        .batches()
        .iter()
        .map(|batch| {
            batch
                .iter()
                .map(|t| {
                    let base = BuildExecutionPlan::task_base_name(t);
                    let label = plan.label_for_task(t, &build_opts);
                    if label == "build" || label == "build:full" {
                        base.to_string()
                    } else {
                        format!("{} ({})", base, label)
                    }
                })
                .collect()
        })
        .collect();
    if !quiet {
        let pkg_word = if total_packages == 1 { "package" } else { "packages" };
        if total_batches == 1 {
            // Single batch: one line is enough — list the packages directly.
            info!(
                verb = "Planning",
                event = "plan.summary",
                total_packages = total_packages,
                total_batches = total_batches,
                "{} {}: {}",
                total_packages,
                pkg_word,
                batch_entries[0].join(", "),
            );
        } else {
            // For multi-batch plans, the planning line shows totals only.
            // Each batch's contents are announced at execution time via the
            // "Batch N/M: …" line in the loop below, which interleaves with
            // the actual build progress.
            info!(
                verb = "Planning",
                event = "plan.summary",
                total_packages = total_packages,
                total_batches = total_batches,
                "{} {} across {} batches",
                total_packages, pkg_word, total_batches
            );
            // Structured per-batch entries still go to the file log for
            // post-mortem analysis.
            for (idx, entries) in batch_entries.iter().enumerate() {
                tracing::debug!(
                    event = "plan.batch",
                    batch_num = idx + 1,
                    total_batches = total_batches,
                    tasks = %entries.join(", "),
                    "plan batch contents",
                );
            }
        }
    }

    let plan = Arc::new(plan);
    let foundry = Arc::new(Foundry::new(config.clone()));
    let resources = resolve::summarize_build_resources(config);
    // configure_lock = 1 permit (serializes autotools-style configure scripts).
    // compile_lock   = total_cpus permits; each compile stage takes N permits
    //                  matching its declared CPU usage, so the pool stays at
    //                  exactly total_cpus in flight across the whole batch.
    let configure_lock = Arc::new(Semaphore::new(1));
    let compile_lock = Arc::new(Semaphore::new(resources.total_cpus));

    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("open database: {}", e)))?;

    // ── Crash recovery ──────────────────────────────────────────────
    crate::delivery::recover_if_needed(&db).await?;

    // ── Begin delivery transaction ──────────────────────────────────
    let command_str = format!("install {}", targets.join(" "));
    let tx_id = crate::delivery::begin_delivery(&db, &command_str).await?;

    let session = SessionContext {
        id: format!(
            "{:x}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ),
        command: command_str.clone(),
    };

    // ── Compute plan fingerprints & CAS resolution ──────────────────
    let plan_fps = PlanFingerprints::compute(&plan, &foundry)?;
    let cas_store = CasStore::new(config.general.store_dir.clone());

    // Pre-compute every part name that will be deployed across all batches,
    // so that per-batch runtime-dependency checks can distinguish "scheduled
    // in a later batch" from genuinely missing dependencies.
    let mut all_upcoming_outputs: HashSet<String> = HashSet::new();
    for batch in plan.batches() {
        for task in batch {
            let base = BuildExecutionPlan::task_base_name(task);
            let plan_path = plan
                .plan_path_for_task(task)
                .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)));
            if let Some(path) = plan_path {
                if let Ok(manifest) = PlanManifest::from_file(path) {
                    for pn in manifest_part_names(&manifest) {
                        all_upcoming_outputs.insert(pn);
                    }
                }
            }
        }
    }

    for (batch_idx, batch) in plan.batches().iter().enumerate() {
        if !quiet && total_batches > 1 {
            let bases: Vec<&str> = batch
                .iter()
                .map(|t| BuildExecutionPlan::task_base_name(t))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            // "Forging" is the gerund verb; the message conveys which
            // batch and its members.
            info!(
                verb = "Building",
                event = "build.batch_started",
                batch_num = batch_idx + 1,
                total_batches = total_batches,
                "batch {}/{}: {}",
                batch_idx + 1,
                total_batches,
                bases.join(", ")
            );
        }

        // Collect which bases in this batch have CAS hits.
        // When --force is set, skip CAS lookup entirely so that forge
        // and seal always run from scratch.
        let mut cas_hit_bases: HashSet<String> = HashSet::new();
        if !force {
            let mut bases_seen = HashSet::new();
            for task in batch {
                let base = BuildExecutionPlan::task_base_name(task).to_string();
                if !bases_seen.insert(base.clone()) {
                    continue;
                }
                // Check if ALL parts of this plan exist in CAS.
                let plan_path = plan
                    .plan_path_for_task(task)
                    .and_then(|p| PlanManifest::from_file(p).ok());
                if let Some(ref manifest) = plan_path {
                    let fp_key = if let Some(fp) = plan_fps.get(task) {
                        fp.clone()
                    } else {
                        continue;
                    };
                    trace!(event = "fingerprint.cas_check", plan_name = %base, fp_key = %&fp_key[..8], "CAS check key computed");
                    let part_names = manifest_part_names(manifest);
                    let all_in_cas = part_names
                        .iter()
                        .all(|pn| cas_store.resolve(pn, &fp_key).is_some());
                    if all_in_cas && !part_names.is_empty() {
                        info!(event = "cas.hit", plan_name = %base, "Using cached build");
                        debug!(event = "cas.found", plan_name = %base, "Found in cache");
                        cas_hit_bases.insert(base);
                    }
                }
            }
        }

        // 1. Forge all tasks in this batch in parallel.
        //    Skip tasks whose base has a CAS hit.
        let mut build_handles = Vec::new();
        for task in batch {
            let base = BuildExecutionPlan::task_base_name(task).to_string();
            if cas_hit_bases.contains(&base) {
                // CAS hit — skip forge for this task.
                continue;
            }

            let plan = Arc::clone(&plan);
            let foundry = Arc::clone(&foundry);
            let build_opts = build_opts.clone();
            let configure_lock = Arc::clone(&configure_lock);
            let compile_lock = Arc::clone(&compile_lock);
            let config = config.clone();
            let task = task.clone();
            let task_for_handle = task.clone();

            let handle = tokio::spawn(async move {
                let plan_path = plan.plan_path_for_task(&task_for_handle).ok_or_else(|| {
                    WrightError::ForgeError(format!("no path for task {}", task_for_handle))
                })?;
                let base = BuildExecutionPlan::task_base_name(&task_for_handle);
                let is_bootstrap = task_for_handle.ends_with(":bootstrap");
                let bootstrap_excluded = plan.bootstrap_excluded_for(&task_for_handle).to_vec();

                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", base, e)))?;

                let mut extra_env = HashMap::new();
                if is_bootstrap || build_opts.mvp {
                    extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
                    for dep in &bootstrap_excluded {
                        let key = format!(
                            "WRIGHT_BOOTSTRAP_WITHOUT_{}",
                            dep.to_uppercase().replace('-', "_")
                        );
                        extra_env.insert(key, "1".to_string());
                    }
                } else {
                    extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "full".to_string());
                }

                let force = if !is_bootstrap && plan.is_post_bootstrap_full(&task_for_handle) {
                    true
                } else {
                    build_opts.force
                };

                // Bootstrap phase: the foundry's hash-chain checkpoint system
                // handles stage invalidation internally.

                let plan_dir = plan_path
                    .parent()
                    .ok_or_else(|| WrightError::ForgeError("plan path has no parent".into()))?
                    .to_path_buf();

                foundry
                    .build(
                        &manifest,
                        &plan_dir,
                        std::path::Path::new("/"),
                        BuildOptions {
                            stages: build_opts.stages.clone(),
                            force_stage: build_opts.force_stage.clone(),
                            until_stage: build_opts.until_stage.clone(),
                            fetch_only: build_opts.fetch_only,
                            skip_check: build_opts.skip_check,
                            force,
                            clean: build_opts.clean,
                            extra_env,
                            verbose: build_opts.verbose,
                            nproc_per_isolation: config.build.nproc_per_isolation,
                            configure_lock: Some(configure_lock),
                            compile_lock: Some(compile_lock),
                        },
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| WrightError::ForgeError(format!("forge {}: {}", base, e)))
            });
            build_handles.push((task.clone(), handle));
        }

        for (task, handle) in build_handles {
            match handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let _ = crate::delivery::rollback_delivery(&db, tx_id).await;
                    let _ = crate::delivery::cleanup_delivery(&db, tx_id).await;
                    return Err(WrightError::ForgeError(format!(
                        "task '{}' failed: {}",
                        task, e
                    )));
                }
                Err(e) => {
                    let _ = crate::delivery::rollback_delivery(&db, tx_id).await;
                    let _ = crate::delivery::cleanup_delivery(&db, tx_id).await;
                    return Err(WrightError::ForgeError(format!(
                        "task '{}' panicked: {}",
                        task, e
                    )));
                }
            }
        }

        // 2. Seal distinct non-bootstrap bases in this batch.
        //    Skip bases with CAS hits (they don't need re-sealing).
        let mut bases_in_batch: Vec<String> = Vec::new();
        let mut bases_seen: HashSet<String> = HashSet::new();
        for task in batch {
            let base = BuildExecutionPlan::task_base_name(task).to_string();
            if task.ends_with(":bootstrap")
                || !bases_seen.insert(base.clone())
                || cas_hit_bases.contains(&base)
            {
                continue;
            }
            bases_in_batch.push(base);
        }

        let seal_word = if bases_in_batch.len() == 1 { "package" } else { "packages" };
        let seal_target = if total_batches == 1 {
            format!("{} {}", bases_in_batch.len(), seal_word)
        } else {
            format!(
                "batch {}/{} ({} {})",
                batch_idx + 1,
                total_batches,
                bases_in_batch.len(),
                seal_word,
            )
        };
        let _seal_span = crate::cli_span!("Sealing", "{}", seal_target);

        for base in &bases_in_batch {
            let plan_path = plan
                .plan_path_for_task(base)
                .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                .ok_or_else(|| WrightError::ForgeError(format!("no plan path for {}", base)))?;
            let manifest = PlanManifest::from_file(plan_path)
                .map_err(|e| WrightError::ForgeError(format!("parse plan {}: {}", base, e)))?;

            crate::seal::package_manifest(&manifest, config, false, force)
                .await
                .map_err(|e| WrightError::ForgeError(format!("seal {}: {}", base, e)))?;

            // Store freshly-sealed parts in CAS.
            if let Some(fp) = plan_fps.get(base) {
                let part_names = manifest_part_names(&manifest);
                for pn in &part_names {
                    if let Ok(Some(resolved)) = part_store.resolve(pn).await
                        && let Err(e) = cas_store.store(&resolved.path, pn, fp)
                    {
                        warn!(event = "cas.store_failed", part_name = %pn, error = %e, "Failed to store part in CAS");
                    }
                }
            }
        }

        // 3. Deploy this wave.
        //    Also restore CAS parts for bases with CAS hits (they weren't
        //    freshly sealed above, so we need to make them available in
        //    parts_dir for the deploy step).
        if !bases_in_batch.is_empty() || !cas_hit_bases.is_empty() {
            // Restore CAS parts for bases with CAS hits.
            for base in &cas_hit_bases {
                if let Some(fp) = plan_fps.get(base) {
                    let plan_path = plan
                        .plan_path_for_task(base)
                        .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                        .ok_or_else(|| {
                            WrightError::ForgeError(format!("no plan path for {}", base))
                        })?;
                    let Ok(manifest) = PlanManifest::from_file(plan_path) else {
                        continue;
                    };
                    let part_names = manifest_part_names(&manifest);
                    for pn in &part_names {
                        if let Some(cas_path) = cas_store.resolve(pn, fp) {
                            // Copy CAS archive to parts_dir so the deploy
                            // step can find it via the normal part_store.
                            let ver = manifest.metadata.version.as_deref().unwrap_or("");
                            let full_name = if manifest.metadata.epoch > 0 {
                                if ver.is_empty() {
                                    format!(
                                        "{}-{}:{}-{}.wright.tar.zst",
                                        pn,
                                        manifest.metadata.epoch,
                                        manifest.metadata.release,
                                        manifest.metadata.arch
                                    )
                                } else {
                                    format!(
                                        "{}-{}:{}-{}-{}.wright.tar.zst",
                                        pn,
                                        manifest.metadata.epoch,
                                        ver,
                                        manifest.metadata.release,
                                        manifest.metadata.arch
                                    )
                                }
                            } else if ver.is_empty() {
                                format!(
                                    "{}-{}-{}.wright.tar.zst",
                                    pn, manifest.metadata.release, manifest.metadata.arch
                                )
                            } else {
                                format!(
                                    "{}-{}-{}-{}.wright.tar.zst",
                                    pn, ver, manifest.metadata.release, manifest.metadata.arch
                                )
                            };
                            let dest = config.general.parts_dir.join(full_name);
                            if !dest.exists() {
                                let _ = std::fs::copy(&cas_path, &dest);
                            }
                        }
                    }
                }
            }

            let mut archive_paths: Vec<PathBuf> = Vec::new();
            let mut explicit: HashSet<String> = HashSet::new();

            // Collect parts from newly-sealed bases.
            for base in &bases_in_batch {
                let plan_path = plan
                    .plan_path_for_task(base)
                    .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                    .ok_or_else(|| WrightError::ForgeError(format!("no plan path for {}", base)))?;
                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::ForgeError(format!("parse plan {}: {}", base, e)))?;

                let part_names = manifest_part_names(&manifest);
                for pn in &part_names {
                    let resolved = part_store
                        .resolve(pn)
                        .await
                        .map_err(|e| {
                            WrightError::PartError(format!(
                                "resolve part {} after packaging: {}",
                                pn, e
                            ))
                        })?
                        .ok_or_else(|| {
                            WrightError::PartNotFound(format!(
                                "part {} not found after sealing",
                                pn
                            ))
                        })?;
                    archive_paths.push(resolved.path);
                    if explicit_plan_names.contains(base) {
                        explicit.insert(pn.clone());
                    }
                }
            }

            // Collect parts from CAS-hit bases.
            for base in &cas_hit_bases {
                if !bases_in_batch.contains(base) {
                    let plan_path = plan
                        .plan_path_for_task(base)
                        .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                        .ok_or_else(|| {
                            WrightError::ForgeError(format!("no plan path for {}", base))
                        })?;
                    let manifest = PlanManifest::from_file(plan_path).map_err(|e| {
                        WrightError::ForgeError(format!("parse plan {}: {}", base, e))
                    })?;

                    let part_names = manifest_part_names(&manifest);
                    for pn in &part_names {
                        let resolved = part_store
                            .resolve(pn)
                            .await
                            .map_err(|e| {
                                WrightError::PartError(format!(
                                    "resolve part {} after CAS restore: {}",
                                    pn, e
                                ))
                            })?
                            .ok_or_else(|| {
                                WrightError::PartNotFound(format!(
                                    "CAS part {} not found after restore",
                                    pn
                                ))
                            })?;
                        if !archive_paths.contains(&resolved.path) {
                            archive_paths.push(resolved.path);
                            if explicit_plan_names.contains(base) {
                                explicit.insert(pn.clone());
                            }
                        }
                    }
                }
            }

            if !archive_paths.is_empty() {
                // Mark delivery as READY (all forge+seal done) before applying.
                crate::delivery::delivery_ready(&db, tx_id).await?;
                crate::delivery::begin_applying(&db, tx_id).await?;

                let part_word = if archive_paths.len() == 1 { "part" } else { "parts" };
                let deploy_target = if total_batches == 1 {
                    format!("{} {}", archive_paths.len(), part_word)
                } else {
                    format!(
                        "batch {}/{} ({} {})",
                        batch_idx + 1,
                        total_batches,
                        archive_paths.len(),
                        part_word,
                    )
                };
                let _deploy_span = crate::cli_span!("Deploying", "{}", deploy_target);

                let result = crate::transaction::deploy_parts_with_explicit_targets(
                    &db,
                    &archive_paths,
                    &explicit,
                    root_dir,
                    part_store,
                    force,
                    false,
                    Some(&all_upcoming_outputs),
                    session.clone(),
                )
                .await;

                match result {
                    Ok(()) => {}
                    Err(e) => {
                        crate::delivery::rollback_delivery(&db, tx_id).await?;
                        let _ = crate::delivery::cleanup_delivery(&db, tx_id).await;
                        return Err(WrightError::DeployError(format!("deploy batch: {}", e)));
                    }
                }
            }
        }
    }

    // ── Mark delivery as COMPLETED ──────────────────────────────────
    crate::delivery::complete_delivery(&db, tx_id).await?;
    let _ = crate::delivery::cleanup_delivery(&db, tx_id).await;

    // Rule C: terminal completion line for the entire install workflow.
    if !quiet {
        let elapsed = workflow_t0.elapsed().as_secs_f64();
        info!(
            verb = "Finished",
            event = "install.completed",
            elapsed_secs = elapsed,
            "install in {}",
            crate::foundry::logging::format_duration(elapsed),
        );
    }

    Ok(())
}

async fn register_folio_assumptions(
    db_path: &Path,
    provides: &[folio::FolioProvide],
) -> Result<()> {
    if provides.is_empty() {
        return Ok(());
    }

    let db = InstalledDb::open(db_path).await.map_err(|e| {
        WrightError::DatabaseError(format!(
            "failed to open database for folio assumptions: {}",
            e
        ))
    })?;
    for provide in provides {
        db.provide_part(&provide.name, &provide.version)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to assume {}: {}", provide.name, e))
            })?;
    }
    Ok(())
}

fn manifest_part_names(manifest: &PlanManifest) -> Vec<String> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![manifest.metadata.name.clone()],
    }
}

