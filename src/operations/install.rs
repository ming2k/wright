use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Result, WrightError};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::{debug, info, trace};

use crate::config::GlobalConfig;
use crate::database::{InstalledDb, SessionContext};
use crate::delivery::store::CasStore;
use crate::forge::Forger;
use crate::part::folio;
use crate::part::store::LocalPartStore;
use crate::plan::manifest::{OutputConfig, PlanManifest};
use crate::resolve::{
    self, DepDomain, ForgeExecutionPlan, ForgeOptions, MatchPolicy, ResolveOptions,
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
    /// instead of default ForgeOptions (used by `wright build`).
    pub forge_opts: Option<ForgeOptions>,
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
    fn compute(plan: &ForgeExecutionPlan, forger: &Forger) -> Result<Self> {
        let mut fingerprints: HashMap<String, String> = HashMap::new();

        // Process batch by batch so that dependency fingerprints are available
        // when computing closure fingerprints for later batches.
        for batch in plan.batches() {
            for task in batch {
                let base = ForgeExecutionPlan::task_base_name(task);
                let plan_path = plan
                    .plan_path_for_task(task)
                    .ok_or_else(|| WrightError::ForgeError(format!("no path for task {}", task)))?;
                let manifest = PlanManifest::from_file(plan_path)
                    .map_err(|e| WrightError::ForgeError(format!("read plan {}: {}", base, e)))?;

                let build_key = forger.compute_build_key(&manifest)?;

                // Collect fingerprints of build dependencies.
                let dep_names = plan.deps_for_task(task);
                let mut dep_fps: HashMap<String, String> = HashMap::new();
                for dep_name in dep_names {
                    let dep_base = ForgeExecutionPlan::task_base_name(dep_name);
                    if let Some(fp) = fingerprints.get(dep_base) {
                        dep_fps.insert(dep_base.to_string(), fp.clone());
                    }
                }

                let closure_fp = CasStore::compute_closure_fingerprint(&build_key, &dep_fps);
                trace!("{} closure_fp={}", base, &closure_fp[..8]);

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
                        trace!("{}:bootstrap fp={}", base, &bootstrap_fp[..8]);
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
        forge_opts,
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

    let build_opts = forge_opts.unwrap_or_else(|| ForgeOptions {
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

    let plan_dirs = resolve::plan_search_dirs(config);
    let explicit_plan_names = resolve_explicit_plan_names(&plan_dirs, &targets)
        .map_err(|e| WrightError::ForgeError(format!("explicit plan names: {}", e)))?;

    let plan = create_execution_plan(config, build_set, &build_opts, dep_domain)
        .map_err(|e| WrightError::ForgeError(format!("create_execution_plan: {}", e)))?;

    let total_packages = plan.build_set().len();
    let total_batches = plan.batches().len();
    if !quiet {
        info!(
            "build plan: {} package{} in {} batch{}",
            total_packages,
            if total_packages == 1 { "" } else { "s" },
            total_batches,
            if total_batches == 1 { "" } else { "es" }
        );
        for (idx, batch) in plan.batches().iter().enumerate() {
            let entries: Vec<String> = batch
                .iter()
                .map(|t| {
                    let base = ForgeExecutionPlan::task_base_name(t);
                    let label = plan.label_for_task(t, &build_opts);
                    if label == "build" || label == "build:full" {
                        base.to_string()
                    } else {
                        format!("{} ({})", base, label)
                    }
                })
                .collect();
            info!("  batch {}/{}: {}", idx + 1, total_batches, entries.join(", "));
        }
    }

    let plan = Arc::new(plan);
    let forger = Arc::new(Forger::new(config.clone()));
    let configure_lock = Arc::new(Mutex::new(()));
    let compile_lock = Arc::new(Mutex::new(()));
    let _resources = resolve::summarize_forge_resources(config);

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
    let plan_fps = PlanFingerprints::compute(&plan, &forger)?;
    let cas_store = CasStore::new(config.general.store_dir.clone());

    for (batch_idx, batch) in plan.batches().iter().enumerate() {
        if !quiet {
            let bases: Vec<&str> = batch
                .iter()
                .map(|t| ForgeExecutionPlan::task_base_name(t))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            info!(
                "Build batch {}/{}: {}",
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
                let base = ForgeExecutionPlan::task_base_name(task).to_string();
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
                    trace!("{} CAS check key={}", base, &fp_key[..8]);
                    let part_names = manifest_part_names(manifest);
                    let all_in_cas = part_names
                        .iter()
                        .all(|pn| cas_store.resolve(pn, &fp_key).is_some());
                    if all_in_cas && !part_names.is_empty() {
                        info!("using cached build for {}", base);
                        debug!("{} found in cache", base);
                        cas_hit_bases.insert(base);
                    }
                }
            }
        }

        // 1. Forge all tasks in this batch in parallel.
        //    Skip tasks whose base has a CAS hit.
        let mut build_handles = Vec::new();
        for task in batch {
            let base = ForgeExecutionPlan::task_base_name(task).to_string();
            if cas_hit_bases.contains(&base) {
                // CAS hit — skip forge for this task.
                continue;
            }

            let plan = Arc::clone(&plan);
            let forger = Arc::clone(&forger);
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
                let base = ForgeExecutionPlan::task_base_name(&task_for_handle);
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

                // Bootstrap phase: the forger's hash-chain checkpoint system
                // handles stage invalidation internally.

                let plan_dir = plan_path
                    .parent()
                    .ok_or_else(|| WrightError::ForgeError("plan path has no parent".into()))?
                    .to_path_buf();

                forger
                    .build(
                        &manifest,
                        &plan_dir,
                        std::path::Path::new("/"),
                        &build_opts.stages,
                        &build_opts.force_stage,
                        build_opts.until_stage.as_deref(),
                        build_opts.fetch_only,
                        build_opts.skip_check,
                        force,
                        &extra_env,
                        build_opts.verbose,
                        config.build.nproc_per_isolation,
                        Some(configure_lock),
                        Some(compile_lock),
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
            let base = ForgeExecutionPlan::task_base_name(task).to_string();
            if task.ends_with(":bootstrap")
                || !bases_seen.insert(base.clone())
                || cas_hit_bases.contains(&base)
            {
                continue;
            }
            bases_in_batch.push(base);
        }

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
                        && let Err(e) = cas_store.store(&resolved.path, pn, fp) {
                            tracing::warn!("Failed to store {} in CAS: {}", pn, e);
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

                let result = crate::transaction::deploy_parts_with_explicit_targets(
                    &db,
                    &archive_paths,
                    &explicit,
                    root_dir,
                    part_store,
                    force,
                    false,
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

    Ok(())
}

async fn register_folio_assumptions(
    db_path: &Path,
    assumptions: &[folio::FolioAssume],
) -> Result<()> {
    if assumptions.is_empty() {
        return Ok(());
    }

    let db = InstalledDb::open(db_path).await.map_err(|e| {
        WrightError::DatabaseError(format!(
            "failed to open database for folio assumptions: {}",
            e
        ))
    })?;
    for assume in assumptions {
        db.assume_part(&assume.name, &assume.version)
            .await
            .map_err(|e| {
                WrightError::DatabaseError(format!("failed to assume {}: {}", assume.name, e))
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
