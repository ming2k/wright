use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::builder::logging;
use crate::builder::mvp::{cycle_candidates_for, find_cycles, format_cycle_path, pick_candidate};
use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError, WrightResultExt};
use crate::part::fhs;
use crate::part::part;
use crate::plan::manifest::{OutputConfig, PlanManifest};

use super::BuildOptions;

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_builds(
    config: &GlobalConfig,
    name_to_path: &HashMap<String, PathBuf>,
    deps_map: &HashMap<String, Vec<String>>,
    build_set: &HashSet<String>,
    opts: &BuildOptions,
    bootstrap_excluded: &HashMap<String, Vec<String>>,
    session_hash: Option<&str>,
    session_completed: &HashSet<String>,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<std::result::Result<String, (String, WrightError)>>(100);
    let completed = Arc::new(Mutex::new(session_completed.clone()));
    let in_progress = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_count = Arc::new(Mutex::new(0usize));
    let base_root = PathBuf::from("/");

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());
    let compile_lock = Arc::new(Mutex::new(()));
    let bootstrap_excluded = Arc::new(bootstrap_excluded.clone());
    let session_hash_arc = Arc::new(session_hash.map(|s| s.to_string()));

    let resources = super::summarize_build_resources(config);
    let total_cpus = resources.total_cpus;
    let actual_isolations = resources.concurrent_tasks;

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().await;
            let prog = in_progress.lock().await;
            let fail = failed_set.lock().await;

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let all_deps_met = opts.checksum
                        || deps_map
                            .get(name)
                            .expect("build node exists")
                            .iter()
                            .filter(|d| build_set.contains(*d))
                            .all(|d| comp.contains(d));

                    if all_deps_met {
                        ready_to_launch.push(name.clone());
                    }
                }
            }
        }

        let base_active = in_progress.lock().await.len();
        let free_slots = actual_isolations.saturating_sub(base_active);
        let launch_batch: Vec<_> = ready_to_launch.into_iter().take(free_slots).collect();
        let planned_active = base_active + launch_batch.len();

        for (launch_idx, name) in launch_batch.into_iter().enumerate() {
            {
                let mut in_progress_guard = in_progress.lock().await;
                in_progress_guard.insert(name.clone());
            }

            let dynamic_nproc_cap = if let Some(n) = opts.nproc_per_isolation {
                Some(n)
            } else {
                let base_share = (total_cpus / planned_active.max(1)).max(1);
                let remainder = total_cpus % planned_active.max(1);
                let active_position = base_active + launch_idx + 1;
                let share = if active_position <= remainder {
                    base_share + 1
                } else {
                    base_share
                };
                Some(share as u32)
            };

            info!("{}", logging::build_started(&name));

            let tx_clone = tx.clone();
            let name_clone = name.clone();
            let path = name_to_path.get(&name).expect("plan path exists").clone();
            let builder_clone = builder.clone();
            let config_clone = config_arc.clone();
            let compile_lock_clone = compile_lock.clone();
            let bootstrap_excluded_clone = bootstrap_excluded.clone();
            let base_root_clone = base_root.clone();

            let bootstrap_excl = bootstrap_excluded_clone
                .get(&name)
                .cloned()
                .unwrap_or_default();
            let is_post_bootstrap =
                !name.ends_with(":bootstrap") && build_set.contains(&format!("{}:bootstrap", name));
            let mut effective_opts = opts.clone();
            if is_post_bootstrap {
                effective_opts.force = true;
            }
            if actual_isolations == 1 && !opts.quiet {
                effective_opts.verbose = true;
            } else if actual_isolations > 1 && !opts.verbose {
                effective_opts.verbose = false;
            }
            effective_opts.nproc_per_isolation = dynamic_nproc_cap;

            let spinner = if actual_isolations > 1 && build_set.len() > 1 && !opts.quiet {
                let pb = crate::util::progress::MULTI.add(indicatif::ProgressBar::new_spinner());
                pb.set_style(
                    indicatif::ProgressStyle::default_spinner()
                        .template("{spinner:.cyan} {prefix}: {msg}")
                        .expect("valid spinner template"),
                );
                pb.set_prefix(name.clone());
                pb.set_message("starting");
                pb.enable_steady_tick(std::time::Duration::from_millis(100));
                Some(pb)
            } else {
                None
            };

            tokio::spawn(async move {
                let manifest = match PlanManifest::from_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        if let Some(ref pb) = spinner {
                            pb.finish_and_clear();
                        }
                        let _ = tx_clone.send(Err((name_clone, e))).await;
                        return;
                    }
                };
                let res = build_one(
                    &builder_clone,
                    &manifest,
                    &path,
                    &config_clone,
                    &base_root_clone,
                    &effective_opts,
                    &bootstrap_excl,
                    compile_lock_clone.clone(),
                    spinner.clone(),
                ).await;

                match res {
                    Ok(_) => {
                        if let Some(ref pb) = spinner {
                            pb.finish_and_clear();
                        }
                        let _ = tx_clone.send(Ok(name_clone)).await;
                    }
                    Err(e) => {
                        if let Some(ref pb) = spinner {
                            pb.finish_and_clear();
                        }
                        error!("Failed to process {}: {:#}", name_clone, e);
                        let _ = tx_clone.send(Err((name_clone, e))).await;
                    }
                }
            });
        }

        let finished_count = completed.lock().await.len() + *failed_count.lock().await;
        if in_progress.lock().await.is_empty() && finished_count == build_set.len() {
            break;
        }

        if in_progress.lock().await.is_empty() && finished_count < build_set.len() {
            let mut message =
                String::from("Deadlock detected or dependency missing from plan set:\n");
            let comp = completed.lock().await;
            let prog = in_progress.lock().await;
            let fail = failed_set.lock().await;

            for name in build_set {
                if !comp.contains(name) && !prog.contains(name) && !fail.contains(name) {
                    let missing: Vec<_> = deps_map
                        .get(name)
                        .expect("build node exists")
                        .iter()
                        .filter(|d| build_set.contains(*d) && !comp.contains(*d))
                        .cloned()
                        .collect();
                    message.push_str(&format!(
                        "  - {} is waiting for: {}\n",
                        name,
                        missing.join(", ")
                    ));
                }
            }
            return Err(WrightError::BuildError(message));
        }

        match rx.recv().await {
            None => {
                return Err(WrightError::BuildError(
                    "isolation task disconnected unexpectedly".to_string(),
                ));
            }
            Some(Ok(name)) => {
                in_progress.lock().await.remove(&name);
                complete_build_task(
                    config,
                    session_hash_arc.as_deref(),
                    &completed,
                    &name,
                    opts.quiet,
                ).await;
            }
            Some(Err((name, _))) => {
                in_progress.lock().await.remove(&name);
                failed_set.lock().await.insert(name.clone());
                *failed_count.lock().await += 1;
                if !opts.checksum {
                    return Err(WrightError::BuildError(format!(
                        "Construction failed due to error in {}",
                        name
                    )));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().await;
    let final_completed = completed.lock().await.len();

    if final_failed > 0 {
        warn!(
            "Construction finished with {} successes and {} failures.",
            final_completed, final_failed
        );
        if !opts.checksum {
            return Err(WrightError::BuildError(
                "Some parts failed to manufacture.".to_string(),
            ));
        }
    } else {
        info!(
            "Completed all {} build tasks successfully.",
            final_completed
        );
    }

    Ok(())
}

async fn complete_build_task(
    config: &GlobalConfig,
    session_hash: Option<&str>,
    completed: &Arc<Mutex<HashSet<String>>>,
    name: &str,
    quiet: bool,
) {
    if let Some(hash) = session_hash {
        if let Ok(db) = InstalledDb::open(&config.general.installed_db_path).await {
            let _ = db.mark_session_completed(hash, name).await;
        }
    }
    completed.lock().await.insert(name.to_string());
    if !quiet {
        info!("{}", logging::build_finished(name));
    }
}

pub(super) fn lint_dependency_graph(
    plans_to_build: &HashSet<PathBuf>,
    all_plans: &HashMap<String, PathBuf>,
    build_dep_map: impl FnOnce(
        &HashSet<PathBuf>,
        bool,
        bool,
        HashMap<String, super::RebuildReason>,
        &HashMap<String, PathBuf>,
    ) -> Result<crate::builder::mvp::PlanGraph>,
) -> Result<()> {
    let graph = build_dep_map(plans_to_build, false, false, HashMap::new(), all_plans)?;
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
        let candidates = cycle_candidates_for(cycle, &graph);
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

#[allow(clippy::too_many_arguments)]
async fn build_one(
    builder: &Builder,
    manifest: &PlanManifest,
    manifest_path: &Path,
    config: &GlobalConfig,
    base_root: &Path,
    opts: &BuildOptions,
    bootstrap_excl: &[String],
    compile_lock: Arc<Mutex<()>>,
    progress: Option<indicatif::ProgressBar>,
) -> Result<()> {
    if opts.checksum {
        builder
            .update_hashes(manifest, manifest_path).await
            .context("failed to update hashes")?;
        info!("Updated source hashes in plan {}", manifest.plan.name);
        return Ok(());
    }

    if opts.lint {
        println!(
            "valid plan: {} {}-{}",
            manifest.plan.name, manifest.plan.version, manifest.plan.release
        );
        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            for sub_name in parts.keys() {
                if sub_name != &manifest.plan.name {
                    println!("  sub-part: {}", sub_name);
                }
            }
        }
        return Ok(());
    }

    if opts.clean {
        builder
            .clean(manifest).await
            .context("failed to clean workspace")?;
    }

    let output_dir = if config.general.parts_dir.exists()
        || tokio::fs::create_dir_all(&config.general.parts_dir).await.is_ok()
    {
        config.general.parts_dir.clone()
    } else {
        std::env::current_dir().map_err(WrightError::IoError)?
    };

    if !opts.force && opts.resume.is_none() && opts.stages.is_empty() && !opts.fetch_only {
        let part_name = manifest.part_filename();
        let existing = output_dir.join(&part_name);
        let all_exist = existing.exists()
            && match manifest.outputs {
                Some(OutputConfig::Multi(ref parts)) => parts
                    .iter()
                    .filter(|(name, _)| *name != &manifest.plan.name)
                    .all(|(sub_name, sub_part)| {
                        let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                        output_dir.join(sub_manifest.part_filename()).exists()
                    }),
                _ => true,
            };
        if all_exist && existing.exists() {
            info!("{}", logging::plan_skipped_existing(&manifest.plan.name));
            return Ok(());
        }
    }

    let mut extra_env = std::collections::HashMap::new();
    if !bootstrap_excl.is_empty() || opts.mvp {
        if manifest.mvp.is_none() && !bootstrap_excl.is_empty() {
            warn!(
                "Plan '{}' has no mvp.toml; \
                 cannot compute MVP deps for cycle breaking.",
                manifest.plan.name
            );
        }
        extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "mvp".to_string());
        for dep in bootstrap_excl {
            let key = format!(
                "WRIGHT_BOOTSTRAP_WITHOUT_{}",
                dep.to_uppercase().replace('-', "_")
            );
            extra_env.insert(key, "1".to_string());
        }
        if !bootstrap_excl.is_empty() {
            info!(
                "Executing MVP pass for plan {} without {}",
                manifest.plan.name,
                bootstrap_excl.join(", ")
            );
        } else {
            info!("Executing MVP pass for plan {}", manifest.plan.name);
        }
    }

    if !extra_env.contains_key("WRIGHT_BUILD_PHASE") {
        extra_env.insert("WRIGHT_BUILD_PHASE".to_string(), "full".to_string());
    }
    let plan_dir = manifest_path.parent().expect("plan parent").to_path_buf();
    let result = builder.build(
        manifest,
        &plan_dir,
        base_root,
        &opts.stages,
        opts.fetch_only,
        opts.skip_check,
        &extra_env,
        opts.verbose,
        opts.nproc_per_isolation,
        Some(compile_lock),
        progress,
    ).await?;

    let has_fabricate_stage = manifest.outputs.is_some();
    let produces_output = opts.stages.is_empty() || has_fabricate_stage; // Simplified for now
    if produces_output && !opts.fetch_only {
        if !manifest.options.skip_fhs_check {
            fhs::validate(&result.output_dir, &manifest.plan.name)?;
        }
        let part_path = part::create_part(&result.output_dir, manifest, &output_dir)?;
        info!("{}", logging::plan_packed(&manifest.plan.name, &part_path));
        if opts.print_parts {
            println!("{}", part_path.display());
        }
        register_in_archive_db(&config.general.archive_db_path, &part_path).await;

        if let Some(OutputConfig::Multi(ref parts)) = manifest.outputs {
            for (sub_name, sub_part) in parts {
                if sub_name == &manifest.plan.name {
                    continue;
                }
                let sub_part_dir = result.split_part_dirs.get(sub_name).ok_or_else(|| {
                    WrightError::BuildError(format!("missing sub-part output_dir for '{}'", sub_name))
                })?;
                if !manifest.options.skip_fhs_check {
                    fhs::validate(sub_part_dir, sub_name)?;
                }
                let sub_manifest = sub_part.to_manifest(sub_name, manifest);
                let sub_part_path = part::create_part(sub_part_dir, &sub_manifest, &output_dir)?;
                info!("{}", logging::plan_packed(sub_name, &sub_part_path));
                if opts.print_parts {
                    println!("{}", sub_part_path.display());
                }
                register_in_archive_db(&config.general.archive_db_path, &sub_part_path).await;
            }
        }
    }

    Ok(())
}

async fn register_in_archive_db(archive_db_path: &Path, part_path: &Path) {
    // Local registration might still be sync if it uses rusqlite, 
    // but ArchiveDb should also be refactored eventually.
    // For now, I'll assume ArchiveDb is still sync but I'll mark this as async.
    let archive_db_path = archive_db_path.to_path_buf();
    let part_path = part_path.to_path_buf();
    
    let _ = tokio::spawn(async move {
        let archive_db = match crate::database::ArchiveDb::open(&archive_db_path).await {
            Ok(db) => db,
            Err(e) => {
                warn!("Failed to open local archive DB: {}", e);
                return;
            }
        };
        
        let partinfo = match tokio::task::spawn_blocking({
            let path = part_path.clone();
            move || part::read_partinfo(&path)
        }).await {
            Ok(Ok(info)) => info,
            Ok(Err(e)) => {
                warn!("Failed to read partinfo for DB registration: {}", e);
                return;
            }
            Err(e) => {
                warn!("Task failed: {}", e);
                return;
            }
        };

        let sha256 = match tokio::task::spawn_blocking({
            let path = part_path.clone();
            move || crate::util::checksum::sha256_file(&path)
        }).await {
            Ok(Ok(hash)) => hash,
            Ok(Err(e)) => {
                warn!("Failed to compute sha256 for DB registration: {}", e);
                return;
            }
            Err(e) => {
                warn!("Task failed: {}", e);
                return;
            }
        };

        let filename = part_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if let Err(e) = archive_db.register_part(&partinfo, filename, &sha256).await {
            warn!("Failed to register in local archive DB: {}", e);
        }
    }).await;
}
