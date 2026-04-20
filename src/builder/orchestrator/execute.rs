use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use tracing::{error, info, warn};

use crate::builder::logging;
use crate::builder::mvp::{cycle_candidates_for, find_cycles, format_cycle_path, pick_candidate};
use crate::builder::Builder;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError, WrightResultExt};
use crate::part::fhs;
use crate::part::part;
use crate::plan::manifest::{FabricateConfig, PlanManifest};

use super::BuildOptions;

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_builds(
    config: &GlobalConfig,
    name_to_path: &HashMap<String, PathBuf>,
    deps_map: &HashMap<String, Vec<String>>,
    build_set: &HashSet<String>,
    opts: &BuildOptions,
    bootstrap_excluded: &HashMap<String, Vec<String>>,
    session_hash: Option<&str>,
    session_completed: &HashSet<String>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel::<std::result::Result<String, (String, WrightError)>>();
    let completed = Arc::new(Mutex::new(session_completed.clone()));
    let in_progress = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_set = Arc::new(Mutex::new(HashSet::<String>::new()));
    let failed_count = Arc::new(Mutex::new(0usize));
    let base_root = PathBuf::from("/");

    let builder = Arc::new(Builder::new(config.clone()));
    let config_arc = Arc::new(config.clone());
    let compile_lock = Arc::new(Mutex::new(()));
    let bootstrap_excluded = Arc::new(bootstrap_excluded.clone());
    let session_hash = Arc::new(session_hash.map(|s| s.to_string()));

    let resources = super::summarize_build_resources(config);
    let total_cpus = resources.total_cpus;
    let actual_isolations = resources.concurrent_tasks;

    loop {
        let mut ready_to_launch = Vec::new();
        {
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

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

        let base_active = in_progress.lock().unwrap().len();
        let free_slots = actual_isolations.saturating_sub(base_active);
        let launch_batch: Vec<_> = ready_to_launch.into_iter().take(free_slots).collect();
        let planned_active = base_active + launch_batch.len();

        for (launch_idx, name) in launch_batch.into_iter().enumerate() {
            {
                let mut in_progress_guard = in_progress.lock().unwrap();
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

            std::thread::spawn(move || {
                let manifest = match PlanManifest::from_file(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        if let Some(ref pb) = spinner {
                            pb.finish_and_clear();
                        }
                        let _ = tx_clone.send(Err((name_clone, e)));
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
                );

                match res {
                    Ok(_) => {
                        if let Some(ref pb) = spinner {
                            pb.finish_and_clear();
                        }
                        let _ = tx_clone.send(Ok(name_clone));
                    }
                    Err(e) => {
                        if let Some(ref pb) = spinner {
                            pb.finish_and_clear();
                        }
                        error!("Failed to process {}: {:#}", name_clone, e);
                        let _ = tx_clone.send(Err((name_clone, e)));
                    }
                }
            });
        }

        let finished_count = completed.lock().unwrap().len() + *failed_count.lock().unwrap();
        if in_progress.lock().unwrap().is_empty() && finished_count == build_set.len() {
            break;
        }

        if in_progress.lock().unwrap().is_empty() && finished_count < build_set.len() {
            let mut message =
                String::from("Deadlock detected or dependency missing from plan set:\n");
            let comp = completed.lock().unwrap();
            let prog = in_progress.lock().unwrap();
            let fail = failed_set.lock().unwrap();

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

        match rx.recv() {
            Err(_) => {
                return Err(WrightError::BuildError(
                    "isolation thread disconnected unexpectedly".to_string(),
                ));
            }
            Ok(Ok(name)) => {
                in_progress.lock().unwrap().remove(&name);
                complete_build_task(
                    config,
                    session_hash.as_deref(),
                    &completed,
                    &name,
                    opts.quiet,
                );
            }
            Ok(Err((name, _))) => {
                in_progress.lock().unwrap().remove(&name);
                failed_set.lock().unwrap().insert(name.clone());
                *failed_count.lock().unwrap() += 1;
                if !opts.checksum {
                    return Err(WrightError::BuildError(format!(
                        "Construction failed due to error in {}",
                        name
                    )));
                }
            }
        }
    }

    let final_failed = *failed_count.lock().unwrap();
    let final_completed = completed.lock().unwrap().len();

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

fn complete_build_task(
    config: &GlobalConfig,
    session_hash: Option<&str>,
    completed: &Arc<Mutex<HashSet<String>>>,
    name: &str,
    quiet: bool,
) {
    if let Some(hash) = session_hash {
        if let Ok(db) = InstalledDb::open(&config.general.installed_db_path) {
            let _ = db.mark_session_completed(hash, name);
        }
    }
    completed.lock().unwrap().insert(name.to_string());
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
                Some(c) if c.pkg == cand.pkg && c.excluded == cand.excluded => "yes",
                _ => "no",
            };
            println!(
                "{} | {} | {} | {}",
                idx + 1,
                cand.pkg,
                cand.excluded.join(", "),
                selected
            );
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_one(
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
            .update_hashes(manifest, manifest_path)
            .context("failed to update hashes")?;
        info!("Updated source hashes in plan {}", manifest.plan.name);
        return Ok(());
    }

    if opts.lint {
        println!(
            "valid plan: {} {}-{}",
            manifest.plan.name, manifest.plan.version, manifest.plan.release
        );
        if let Some(FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
            for sub_name in pkgs.keys() {
                if sub_name != &manifest.plan.name {
                    println!("  sub-part: {}", sub_name);
                }
            }
        }
        return Ok(());
    }

    if opts.clean {
        builder
            .clean(manifest)
            .context("failed to clean workspace")?;
    }

    let output_dir = if config.general.parts_dir.exists()
        || std::fs::create_dir_all(&config.general.parts_dir).is_ok()
    {
        config.general.parts_dir.clone()
    } else {
        std::env::current_dir()?
    };

    if !opts.force && opts.resume.is_none() && opts.stages.is_empty() && !opts.fetch_only {
        let part_name = manifest.part_filename();
        let existing = output_dir.join(&part_name);
        let all_exist = existing.exists()
            && match manifest.fabricate {
                Some(FabricateConfig::Multi(ref pkgs)) => pkgs
                    .iter()
                    .filter(|(name, _)| *name != &manifest.plan.name)
                    .all(|(sub_name, sub_pkg)| {
                        let sub_manifest = sub_pkg.to_manifest(sub_name, manifest);
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
    )?;

    let has_fabricate_stage = manifest.fabricate.is_some()
        || manifest.lifecycle.contains_key("fabricate")
        || manifest.lifecycle.contains_key("pre_fabricate")
        || manifest.lifecycle.contains_key("post_fabricate");
    let explicit_output_stage = opts
        .stages
        .iter()
        .any(|s| s == "fabricate" || s == "post_fabricate");
    let produces_output = opts.stages.is_empty() || (has_fabricate_stage && explicit_output_stage);
    if produces_output && !opts.fetch_only {
        if !manifest.options.skip_fhs_check {
            fhs::validate(&result.pkg_dir, &manifest.plan.name)?;
        }
        let part_path = part::create_part(&result.pkg_dir, manifest, &output_dir)?;
        info!("{}", logging::plan_packed(&manifest.plan.name, &part_path));
        if opts.print_parts {
            println!("{}", part_path.display());
        }
        register_in_archive_db(&config.general.archive_db_path, &part_path);

        if let Some(FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
            for (sub_name, sub_pkg) in pkgs {
                if sub_name == &manifest.plan.name {
                    continue;
                }
                let sub_pkg_dir = result.split_pkg_dirs.get(sub_name).ok_or_else(|| {
                    WrightError::BuildError(format!("missing sub-part pkg_dir for '{}'", sub_name))
                })?;
                if !manifest.options.skip_fhs_check {
                    fhs::validate(sub_pkg_dir, sub_name)?;
                }
                let sub_manifest = sub_pkg.to_manifest(sub_name, manifest);
                let sub_part = part::create_part(sub_pkg_dir, &sub_manifest, &output_dir)?;
                info!("{}", logging::plan_packed(sub_name, &sub_part));
                if opts.print_parts {
                    println!("{}", sub_part.display());
                }
                register_in_archive_db(&config.general.archive_db_path, &sub_part);
            }
        }
    }

    Ok(())
}

fn register_in_archive_db(archive_db_path: &Path, part_path: &Path) {
    let do_register = || -> Result<()> {
        let archive_db = crate::archive::db::ArchiveDb::open(archive_db_path)?;
        let partinfo = part::read_partinfo(part_path)?;
        let sha256 = crate::util::checksum::sha256_file(part_path)?;
        let filename = part_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        archive_db.register_part(&partinfo, filename, &sha256)?;
        Ok(())
    };
    if let Err(e) = do_register() {
        warn!("Failed to register in local archive DB: {}", e);
    }
}
