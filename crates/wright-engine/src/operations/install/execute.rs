use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::GlobalConfig;
use crate::error::{BatchFailures, Result, TaskFailure, WrightError};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{debug, info, trace, warn};

use crate::foundry::{BuildOptions, Foundry};
use crate::resolve::{
    self, BuildExecutionPlan, BuildPlanOptions, MatchPolicy, ResolveOptions, create_execution_plan,
    resolve_build_set, resolve_explicit_plan_names,
};
use wright_part::folio;
use wright_part::store::{LocalPartStore, ResolvedPartVersioned};
use wright_plan::manifest::{OutputConfig, PlanManifest};
use wright_state::cas::CasStore;
use wright_state::database::{InstalledDb, SessionContext};

use super::fingerprints::PlanFingerprints;
use super::request::InstallRequest;
use crate::util::timing::{WorkflowTiming, format_duration};

/// Resolve the archive that a just-sealed (or CAS-restored) plan build must
/// have produced.
///
/// Unlike a bare `part_store.resolve(name)`, this pins the originating plan
/// and exact version/release/epoch, so a foreign archive that merely shares
/// the part name — regardless of its version — is never picked up.
async fn resolve_plan_part(
    part_store: &LocalPartStore,
    manifest: &PlanManifest,
    part_name: &str,
) -> Result<Option<ResolvedPartVersioned>> {
    part_store
        .resolve_from_plan(
            part_name,
            &manifest.metadata.name,
            manifest.metadata.version.as_deref().unwrap_or(""),
            manifest.metadata.release,
            manifest.metadata.epoch,
        )
        .await
        .map_err(|e| WrightError::context(format!("resolve part {}", part_name), e))
}

/// Verify that a CAS entry actually holds the part expected from this plan
/// build. The fingerprint namespace is shared across plans and wright eras,
/// so a filename hit alone does not prove the content belongs to this plan.
fn cas_entry_matches_manifest(cas_path: &Path, manifest: &PlanManifest, part_name: &str) -> bool {
    match wright_part::archive::read_partinfo(cas_path) {
        Ok(info) => {
            info.name == part_name
                && info.plan.name == manifest.metadata.name
                && info.plan.version == manifest.metadata.version.as_deref().unwrap_or("")
                && info.plan.release == manifest.metadata.release
                && info.plan.epoch == manifest.metadata.epoch
        }
        Err(_) => false,
    }
}

/// Run the install workflow and close every run with the per-step timing
/// report — emitted directly on success, deferred into the process-exit
/// failure report on error (see [`WorkflowTiming::log_report`]). The step
/// in flight when an error hit keeps its elapsed time and is marked failed
/// in the report; see [`WorkflowTiming`].
pub async fn execute_install(request: InstallRequest<'_>) -> Result<()> {
    let quiet = request.quiet;
    let timing = WorkflowTiming::new();
    let result = execute_install_inner(request, &timing).await;

    // Rule C: terminal completion line for the entire install workflow.
    // Warnings shown earlier are non-fatal by definition once we reach this
    // line, so surface their count to close the loop for the user.
    if result.is_ok() && !quiet {
        let elapsed = timing.elapsed();
        let warnings = crate::util::logging::cli_warn_count();
        let summary = if warnings == 0 {
            format!("install in {}", format_duration(elapsed))
        } else {
            format!(
                "install in {} ({} {})",
                format_duration(elapsed),
                warnings,
                if warnings == 1 { "warning" } else { "warnings" }
            )
        };
        info!(
            verb = "Finished",
            event = "install.completed",
            elapsed_secs = elapsed.as_secs_f64(),
            warnings,
            "{}",
            summary,
        );
    }

    timing.log_report("install", result.is_ok(), quiet);

    result
}

async fn execute_install_inner(request: InstallRequest<'_>, timing: &WorkflowTiming) -> Result<()> {
    let InstallRequest {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        force,
        clean,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store,
        build_opts,
        run_hooks,
        dry_run,
    } = request;

    if targets.is_empty() {
        return Err(WrightError::ForgeError(
            "no targets specified (pass plan names, folio names prefixed with '@', or paths as arguments or via stdin)".into()
        ));
    }

    let resolve_step = timing.step("resolve");

    let folio_dirs = [config.general.folios_dir.clone()];
    let expansion = folio::expand(&targets, &folio_dirs)?;

    if expansion.plans.is_empty() {
        return Err(WrightError::ForgeError(
            "no plans to forge after expanding folios".into(),
        ));
    }
    let targets = expansion.plans;

    register_folio_assumptions(config, db_path, &expansion.provides).await?;

    let resolve_opts = ResolveOptions {
        deps,
        rdeps,
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
        clean: force || clean,
        force,
        verbose: verbose > 0,
        quiet,
        nproc_per_isolation: config.build.nproc_per_isolation,
        ..Default::default()
    });

    let build_set: Vec<String> = resolve_build_set(config, targets.clone(), resolve_opts.clone())
        .await
        .map_err(|e| WrightError::context("failed to resolve build set", e))?
        .names;

    if build_set.is_empty() {
        if !quiet {
            crate::outln!(
                "{} already installed and up to date (use --force to reinstall)",
                targets.join(", ")
            );
        }
        resolve_step.success();
        return Ok(());
    }

    let plan_dirs = resolve::plan_search_dirs(config);
    let explicit_plan_names = resolve_explicit_plan_names(&plan_dirs, &targets)
        .map_err(|e| WrightError::context("explicit plan names", e))?;

    let plan = create_execution_plan(config, build_set, &build_opts, deps | rdeps)
        .map_err(|e| WrightError::context("create_execution_plan", e))?;

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
        let pkg_word = if total_packages == 1 {
            "package"
        } else {
            "packages"
        };
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
                total_packages,
                pkg_word,
                total_batches
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

    resolve_step.success();

    if dry_run {
        // Preview only: the plan above is fully resolved, so report the exact
        // batches and stop before any forge/seal/deploy side effects.
        crate::outln!("[dry-run] install -> {}", root_dir.display());
        crate::outln!(
            "[dry-run] would forge and deploy {} package(s) across {} batch(es):",
            total_packages,
            total_batches
        );
        for (idx, entries) in batch_entries.iter().enumerate() {
            crate::outln!("  batch {}: {}", idx + 1, entries.join(", "));
        }
        return Ok(());
    }

    let prepare_step = timing.step("prepare");

    let plan = Arc::new(plan);
    let foundry = Arc::new(Foundry::new(config.clone()));
    let resources = resolve::summarize_build_resources(config);
    // configure_lock = 1 permit (serializes autotools-style configure scripts).
    // compile_lock   = total_cpus permits; each compile stage takes N permits
    //                  matching its declared CPU usage, so the pool stays at
    //                  exactly total_cpus in flight across the whole batch.
    let configure_lock = Arc::new(Semaphore::new(1));
    let compile_lock = Arc::new(Semaphore::new(resources.total_cpus));

    let db = InstalledDb::open(db_path, Some(&crate::ledger::dir(config, Some(db_path))))
        .await
        .map_err(|e| WrightError::context("open database", e))?;
    let ledger_dir = crate::ledger::dir(config, Some(db_path));

    // ── Crash recovery ──────────────────────────────────────────────
    wright_state::delivery::recover_if_needed(&db).await?;

    // ── Signal handling ─────────────────────────────────────────────
    // First Ctrl-C / SIGTERM reaps the build subprocess tree and flips the
    // cancel flag so the batch loop rolls back; a second one force-quits.
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    crate::cancellation::spawn_signal_handler(cancel_tx, quiet);

    // ── Begin delivery transaction ──────────────────────────────────
    let command_str = format!("install {}", targets.join(" "));
    let tx_id = wright_state::delivery::begin_delivery(&db, &command_str).await?;

    // Roll back the delivery transaction and abort the moment the user
    // cancels.  Invoked at every sequential boundary (between batches, before
    // sealing each package, before deploy) so a single Ctrl-C bails promptly
    // instead of finishing the current phase first.
    macro_rules! bail_if_cancelled {
        () => {
            if *cancel_rx.borrow() {
                let _ = wright_state::delivery::rollback_delivery(&db, tx_id).await;
                let _ = wright_state::delivery::cleanup_delivery(&db, tx_id).await;
                return Err(WrightError::ForgeError("cancelled by user".into()));
            }
        };
    }

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
            if let Some(path) = plan_path
                && let Ok(manifest) = PlanManifest::from_file(path)
            {
                for pn in manifest_part_names(&manifest) {
                    all_upcoming_outputs.insert(pn);
                }
            }
        }
    }

    prepare_step.success();

    for (batch_idx, batch) in plan.batches().iter().enumerate() {
        bail_if_cancelled!();

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

        let forge_step = timing.step("forge");

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
                    let all_in_cas = part_names.iter().all(|pn| {
                        cas_store
                            .resolve(pn, &fp_key)
                            .is_some_and(|path| cas_entry_matches_manifest(&path, manifest, pn))
                    });
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
        //
        //    Tasks within a batch have no inter-dependencies, so a failing
        //    task never interrupts its siblings: each failure is announced
        //    the moment it happens, every task runs to completion, and the
        //    failures are settled together once no task is left running.
        let mut join_set: JoinSet<(String, Result<()>)> = JoinSet::new();
        let mut task_ids: HashMap<tokio::task::Id, String> = HashMap::new();
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

            let abort_handle = join_set.spawn(async move {
                let outcome: Result<()> = async {
                    let plan_path = plan.plan_path_for_task(&task_for_handle).ok_or_else(|| {
                        WrightError::ForgeError(format!("no path for task {}", task_for_handle))
                    })?;
                    let base = BuildExecutionPlan::task_base_name(&task_for_handle);
                    let is_bootstrap = task_for_handle.ends_with(":bootstrap");
                    let bootstrap_excluded = plan.bootstrap_excluded_for(&task_for_handle).to_vec();

                    let manifest = PlanManifest::from_file(plan_path)
                        .map_err(|e| WrightError::context(format!("read plan {}", base), e))?;

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
                    // NB: no `forge {base}` context wrap here — batch
                    // settlement attaches the task name to the failure
                    // report itself.
                }
                .await;
                (task_for_handle, outcome)
            });
            task_ids.insert(abort_handle.id(), task.clone());
        }

        // Settle the batch in completion order so a failure is reported the
        // moment it happens rather than when its predecessors finish.
        let mut failures: Vec<TaskFailure> = Vec::new();
        let mut cancelled = false;
        while let Some(joined) = join_set.join_next().await {
            match joined {
                Ok((_task, Ok(()))) => {}
                Ok((task, Err(error))) => {
                    // A build failing because we reaped it on Ctrl-C is a
                    // cancellation, not a genuine build error — swallow it
                    // in favour of the single cancellation outcome below.
                    if *cancel_rx.borrow() {
                        cancelled = true;
                        continue;
                    }
                    // Announce immediately only while siblings are still
                    // running; when this failure empties the batch the
                    // terminal failure report follows at once, so a notice
                    // would print the same failure twice.
                    if !join_set.is_empty() {
                        crate::util::logging::report_task_failure(&task, false, &error);
                    }
                    failures.push(TaskFailure::failed(task, error));
                }
                Err(join_error) => {
                    if *cancel_rx.borrow() {
                        cancelled = true;
                        continue;
                    }
                    // A panicked task never produced its name pairing;
                    // recover the name from the spawn registry.
                    let task = task_ids
                        .remove(&join_error.id())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    if !join_set.is_empty() {
                        crate::util::logging::report_task_failure(&task, true, &join_error);
                    }
                    failures.push(TaskFailure::panicked(
                        task,
                        WrightError::ForgeError(join_error.to_string()),
                    ));
                }
            }
        }

        // The batch is the unit of progression: any failure rolls the
        // delivery transaction back and blocks the next batch.
        if cancelled || !failures.is_empty() {
            let _ = wright_state::delivery::rollback_delivery(&db, tx_id).await;
            let _ = wright_state::delivery::cleanup_delivery(&db, tx_id).await;
            if cancelled {
                return Err(WrightError::ForgeError("cancelled by user".into()));
            }
            BatchFailures::settle(batch_idx + 1, total_batches, failures)?;
        }

        forge_step.success();

        // 2. Seal distinct non-bootstrap bases in this batch.
        //    Skip bases with CAS hits (they don't need re-sealing).
        let seal_step = timing.step("seal");
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

        let seal_word = if bases_in_batch.len() == 1 {
            "package"
        } else {
            "packages"
        };
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

        bail_if_cancelled!();
        for base in &bases_in_batch {
            bail_if_cancelled!();
            let plan_path = plan
                .plan_path_for_task(base)
                .or_else(|| plan.plan_path_for_task(&format!("{}:bootstrap", base)))
                .ok_or_else(|| WrightError::ForgeError(format!("no plan path for {}", base)))?;
            let manifest = PlanManifest::from_file(plan_path)
                .map_err(|e| WrightError::context(format!("parse plan {}", base), e))?;

            crate::seal::package_manifest(&manifest, config, false, force)
                .await
                .map_err(|e| WrightError::context(format!("seal {}", base), e))?;

            // Store freshly-sealed parts in CAS.
            if let Some(fp) = plan_fps.get(base) {
                let part_names = manifest_part_names(&manifest);
                for pn in &part_names {
                    match resolve_plan_part(part_store, &manifest, pn).await {
                        Ok(Some(resolved)) => {
                            if let Err(e) = cas_store.store(&resolved.path, pn, fp) {
                                warn!(event = "cas.store_failed", part_name = %pn, error = %e, "Failed to store part in CAS");
                            }
                        }
                        Ok(None) => {
                            warn!(event = "cas.store_part_missing", plan_name = %base, part_name = %pn, "Sealed part not found in part store; skipping CAS store");
                        }
                        Err(e) => {
                            warn!(event = "cas.store_failed", part_name = %pn, error = %e, "Failed to store part in CAS");
                        }
                    }
                }
            }
        }

        seal_step.success();

        // 3. Deploy this wave.
        //    Also restore CAS parts for bases with CAS hits (they weren't
        //    freshly sealed above, so we need to make them available in
        //    parts_dir for the deploy step).
        let deploy_step = timing.step("deploy");
        bail_if_cancelled!();
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
                            // Restore into the same plan subdirectory the
                            // seal step would have written.
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
                            let plan_dir = config.general.parts_dir.join(&manifest.metadata.name);
                            let dest = plan_dir.join(full_name);
                            if !dest.exists() {
                                let _ = std::fs::create_dir_all(&plan_dir);
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
                    .map_err(|e| WrightError::context(format!("parse plan {}", base), e))?;

                let part_names = manifest_part_names(&manifest);
                for pn in &part_names {
                    let resolved = resolve_plan_part(part_store, &manifest, pn)
                        .await
                        .map_err(|e| {
                            WrightError::context(format!("resolve part {} after packaging", pn), e)
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
                    let manifest = PlanManifest::from_file(plan_path)
                        .map_err(|e| WrightError::context(format!("parse plan {}", base), e))?;

                    let part_names = manifest_part_names(&manifest);
                    for pn in &part_names {
                        let resolved = resolve_plan_part(part_store, &manifest, pn)
                            .await
                            .map_err(|e| {
                                WrightError::context(
                                    format!("resolve part {} after CAS restore", pn),
                                    e,
                                )
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
                wright_state::delivery::delivery_ready(&db, tx_id).await?;
                wright_state::delivery::begin_applying(&db, tx_id).await?;

                let part_word = if archive_paths.len() == 1 {
                    "part"
                } else {
                    "parts"
                };
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
                    run_hooks,
                    session.clone(),
                    &ledger_dir,
                )
                .await;

                match result {
                    Ok(()) => {}
                    Err(e) => {
                        wright_state::delivery::rollback_delivery(&db, tx_id).await?;
                        let _ = wright_state::delivery::cleanup_delivery(&db, tx_id).await;
                        return Err(WrightError::context("deploy batch", e));
                    }
                }
            }
        }
        deploy_step.success();
    }

    // ── Mark delivery as COMPLETED ──────────────────────────────────
    wright_state::delivery::complete_delivery(&db, tx_id).await?;
    let _ = wright_state::delivery::cleanup_delivery(&db, tx_id).await;

    Ok(())
}

async fn register_folio_assumptions(
    config: &GlobalConfig,
    db_path: &Path,
    provides: &[folio::FolioProvide],
) -> Result<()> {
    if provides.is_empty() {
        return Ok(());
    }

    let db = InstalledDb::open(db_path, Some(&crate::ledger::dir(config, Some(db_path))))
        .await
        .map_err(|e| WrightError::context("failed to open database for folio assumptions", e))?;
    for provide in provides {
        db.provide_part(&provide.name, &provide.version)
            .await
            .map_err(|e| WrightError::context(format!("failed to assume {}", provide.name), e))?;
    }
    Ok(())
}

pub(crate) fn manifest_part_names(manifest: &PlanManifest) -> Vec<String> {
    match manifest.outputs {
        Some(OutputConfig::Multi(ref parts)) => parts.iter().map(|(n, _)| n.clone()).collect(),
        _ => vec![manifest.metadata.name.clone()],
    }
}
