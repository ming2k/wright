use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use std::time::Instant;

use tracing::{debug, info, warn};

use crate::database::{Dependency, FileType, InstalledDb, NewPart, Origin, TransactionOperation};
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::archive::PartInfo;
use crate::part::store::LocalPartStore;
use crate::part::version::{self, Version};
use crate::transaction::context::TransactionContext;
use crate::transaction::fs::{collect_file_entries, copy_entries_to_root};
use crate::transaction::hooks::{log_running_hook, read_hooks, run_install_script};

use super::{log_debug_timing, remove_part, upgrade_part};

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanRevision {
    version: String,
    release: u32,
    epoch: u32,
    arch: String,
}

impl PlanRevision {
    fn from_partinfo(partinfo: &PartInfo) -> Self {
        Self {
            version: partinfo.plan.version.clone(),
            release: partinfo.plan.release,
            epoch: partinfo.plan.epoch,
            arch: partinfo.plan.arch.clone(),
        }
    }

    fn label(&self) -> String {
        if self.version.is_empty() {
            format!("{}-{} epoch {}", self.release, self.arch, self.epoch)
        } else {
            format!(
                "{}-{}-{} epoch {}",
                self.version, self.release, self.arch, self.epoch
            )
        }
    }
}

struct InstallCandidate {
    path: PathBuf,
    partinfo: PartInfo,
}

/// Install a batch of parts. All are marked `Origin::Manual`.
/// Use `install_parts_with_explicit_targets` when only a subset are user-requested.
pub async fn install_parts(
    db: &InstalledDb,
    parts: &[PathBuf],
    root_dir: &Path,
    part_store: &LocalPartStore,
    force: bool,
    nodeps: bool,
) -> Result<()> {
    let explicit_targets: HashSet<String> = parts
        .iter()
        .map(|path| part_store.read_part(path))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|resolved| resolved.name)
        .collect();
    install_parts_with_explicit_targets(
        db,
        parts,
        &explicit_targets,
        root_dir,
        part_store,
        force,
        nodeps,
    )
    .await
}

pub async fn install_parts_with_explicit_targets(
    db: &InstalledDb,
    parts: &[PathBuf],
    explicit_targets: &HashSet<String>,
    root_dir: &Path,
    part_store: &LocalPartStore,
    force: bool,
    nodeps: bool,
) -> Result<()> {
    let candidates = read_install_candidates(parts)?;
    validate_plan_output_batches(db, &candidates).await?;

    let mut resolved_map = HashMap::new();

    for candidate in &candidates {
        let resolved = part_store.read_part(&candidate.path)?;
        resolved_map.insert(resolved.name.clone(), resolved);
    }

    if !nodeps {
        warn_about_runtime_dependencies(db, &resolved_map).await?;
    }

    let sorted_names = crate::transaction::dag::sort_dependencies(&resolved_map)?;

    let mut archive_hashes: HashMap<String, String> = HashMap::new();

    for name in sorted_names {
        let explicit_target = explicit_targets.contains(&name);

        if let Some(installed) = db.get_part(&name).await? {
            if explicit_target {
                db.set_origin(&name, Origin::Manual).await?;
            }

            let incoming_hash = if let Some(hash) = archive_hashes.get(&name) {
                Some(hash.clone())
            } else if let Some(part) = resolved_map.get(&name) {
                let hash = crate::util::checksum::sha256_file(&part.path)?;
                archive_hashes.insert(name.clone(), hash.clone());
                Some(hash)
            } else {
                None
            };

            if force || incoming_hash.as_deref() != installed.part_hash.as_deref() {
                info!("Upgrading installed part {} from the current archive", name);
                let part = resolved_map.get(&name).expect("resolved part exists");
                upgrade_part(db, &part.path, root_dir, true, true).await?;
            }
            continue;
        }

        let origin = if explicit_target {
            Origin::Manual
        } else {
            Origin::Dependency
        };
        let part = resolved_map.get(&name).expect("resolved part exists");
        info!("Installing part {} (origin: {})", name, origin);
        install_part_with_origin(db, &part.path, root_dir, force, origin, true).await?;
    }

    Ok(())
}

fn read_install_candidates(parts: &[PathBuf]) -> Result<Vec<InstallCandidate>> {
    parts
        .iter()
        .map(|path| {
            let partinfo = archive::read_partinfo(path)?;
            Ok(InstallCandidate {
                path: path.clone(),
                partinfo,
            })
        })
        .collect()
}

async fn validate_plan_output_batches(
    db: &InstalledDb,
    candidates: &[InstallCandidate],
) -> Result<()> {
    let mut by_plan: BTreeMap<&str, Vec<&InstallCandidate>> = BTreeMap::new();
    for candidate in candidates {
        by_plan
            .entry(candidate.partinfo.plan.name.as_str())
            .or_default()
            .push(candidate);
    }

    for (plan_name, group) in by_plan {
        let first = group.first().expect("non-empty plan group");
        let expected_revision = PlanRevision::from_partinfo(&first.partinfo);
        let mut incoming_outputs = HashSet::new();
        for candidate in group {
            let revision = PlanRevision::from_partinfo(&candidate.partinfo);
            if revision != expected_revision {
                return Err(WrightError::InstallError(format!(
                    "cannot install mixed revisions for plan '{}': '{}' is {}, expected {}",
                    plan_name,
                    candidate.partinfo.name,
                    revision.label(),
                    expected_revision.label()
                )));
            }

            if !incoming_outputs.insert(candidate.partinfo.name.clone()) {
                return Err(WrightError::InstallError(format!(
                    "cannot install duplicate output '{}' for plan '{}' in one batch",
                    candidate.partinfo.name, plan_name
                )));
            }
        }

        if let Some(installed_plan) = db.get_plan(plan_name).await? {
            let installed_outputs = db.get_parts_by_plan(plan_name).await?;
            let installed_revision = PlanRevision {
                version: installed_plan.version,
                release: installed_plan.release as u32,
                epoch: installed_plan.epoch as u32,
                arch: installed_plan.arch,
            };

            if installed_revision != expected_revision {
                let stale_outputs: Vec<_> = installed_outputs
                    .iter()
                    .filter(|part| !incoming_outputs.contains(&part.name))
                    .map(|part| part.name.clone())
                    .collect();
                if !stale_outputs.is_empty() {
                    return Err(WrightError::InstallError(format!(
                        "cannot install plan '{}' {} while installed output(s) from {} would remain: {}; install those outputs in the same batch or use wright apply {}",
                        plan_name,
                        expected_revision.label(),
                        installed_revision.label(),
                        stale_outputs.join(", "),
                        plan_name
                    )));
                }
            }
        }
    }

    Ok(())
}

async fn warn_about_runtime_dependencies(
    db: &InstalledDb,
    resolved_map: &HashMap<String, crate::part::store::ResolvedPart>,
) -> Result<()> {
    let in_batch: HashSet<String> = resolved_map.values().map(|p| p.name.clone()).collect();

    for (name, part) in resolved_map {
        for dep in &part.dependencies {
            let dep = dep.trim();
            if dep.is_empty() {
                warn!(
                    "Part {} declares an empty runtime dependency; continuing install",
                    name
                );
                continue;
            }

            let (dep_ref, constraint) = match version::parse_dependency(dep) {
                Ok(parsed) => parsed,
                Err(e) => {
                    warn!(
                        "Part {} declares invalid runtime dependency '{}': {}; continuing install",
                        name, dep, e
                    );
                    continue;
                }
            };
            let (_, output_name) = version::parse_dep_ref(&dep_ref).to_plan_output();

            if let Some(candidate) = resolved_map.get(&output_name) {
                warn_if_constraint_not_satisfied(
                    name,
                    &output_name,
                    &candidate.version,
                    constraint.as_ref(),
                );
                continue;
            }

            if in_batch.contains(&output_name) {
                continue;
            }

            if let Some(installed) = db.get_part(&output_name).await? {
                let installed_version =
                    if let Some(plan) = db.get_plan_by_id(installed.plan_id).await? {
                        plan.version
                    } else {
                        String::new()
                    };
                warn_if_constraint_not_satisfied(
                    name,
                    &output_name,
                    &installed_version,
                    constraint.as_ref(),
                );
                continue;
            }

            warn!(
                "Runtime dependency {} required by {} is not installed; continuing install",
                output_name, name
            );
        }
    }

    Ok(())
}

fn warn_if_constraint_not_satisfied(
    dependent: &str,
    dependency: &str,
    version: &str,
    constraint: Option<&crate::part::version::VersionConstraint>,
) {
    let Some(constraint) = constraint else {
        return;
    };

    if version.is_empty() {
        return;
    }

    match Version::parse(version) {
        Ok(installed_ver) if constraint.satisfies(&installed_ver) => {}
        _ => {
            warn!(
                "Runtime dependency {} required by {} has version {} which does not satisfy {}; continuing install",
                dependency, dependent, version, constraint
            );
        }
    }
}

pub async fn install_part(
    db: &InstalledDb,
    part_path: &Path,
    root_dir: &Path,
    force: bool,
) -> Result<()> {
    install_part_with_origin(db, part_path, root_dir, force, Origin::Manual, true).await
}

pub async fn install_part_with_origin(
    db: &InstalledDb,
    part_path: &Path,
    root_dir: &Path,
    force: bool,
    origin: Origin,
    run_hooks: bool,
) -> Result<()> {
    let overall_start = Instant::now();

    let staging_dir = root_dir.join("var/lib/wright/staging");
    let _ = tokio::fs::create_dir_all(&staging_dir).await;
    let temp_dir = tempfile::tempdir_in(&staging_dir)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::InstallError(format!("failed to create temp dir: {}", e)))?;

    let mut phase_start = Instant::now();
    let (partinfo, part_hash) = archive::extract_part(part_path, temp_dir.path())?;
    log_debug_timing(
        "install",
        &partinfo.name,
        "archive extraction",
        phase_start.elapsed(),
    );

    for replaced_name in &partinfo.replaces {
        if let Some(existing) = db.get_part(replaced_name).await? {
            info!("Replacing {} with {}", replaced_name, partinfo.name);
            if existing.origin == crate::database::Origin::External {
                // External parts have no filesystem footprint; go through
                // unassume_part so the associated plan record is cleaned up.
                db.unassume_part(replaced_name).await?;
            } else {
                remove_part(db, replaced_name, root_dir, true).await?;
            }
        }
    }

    if !force {
        for conflict_name in &partinfo.conflicts {
            if db.get_part(conflict_name).await?.is_some() {
                return Err(WrightError::DependencyError(format!(
                    "part conflict detected: '{}' conflicts with installed part '{}'. \
                     Please remove it first or use --force.",
                    partinfo.name, conflict_name
                )));
            }
        }

        let reverse_conflicts = db.find_conflicting_parts(&partinfo.name).await?;
        if !reverse_conflicts.is_empty() {
            return Err(WrightError::DependencyError(format!(
                "part conflict detected: installed part(s) {} conflict with '{}'. \
                 Please remove them first or use --force.",
                reverse_conflicts.join(", "),
                partinfo.name
            )));
        }
    }

    if db.get_part(&partinfo.name).await?.is_some() {
        if force {
            debug!(
                "Part {} already installed, attempting upgrade/reinstall",
                partinfo.name
            );
            return upgrade_part(db, part_path, root_dir, true, run_hooks).await;
        }
        return Err(WrightError::PartAlreadyInstalled(partinfo.name.clone()));
    }

    let (hooks_content, hooks) = read_hooks(temp_dir.path());
    phase_start = Instant::now();
    let file_entries = collect_file_entries(temp_dir.path(), &partinfo)?;
    log_debug_timing(
        "install",
        &partinfo.name,
        "file scan and metadata collection",
        phase_start.elapsed(),
    );

    info!("Installing {}: {} files", partinfo.name, file_entries.len());

    phase_start = Instant::now();
    let file_paths: Vec<&str> = file_entries
        .iter()
        .filter(|e| e.file_type == FileType::File)
        .map(|e| e.path.as_str())
        .collect();
    let owners = db.find_owners_batch(&file_paths).await?;

    let mut shadows = Vec::new();
    let mut divert_paths = HashSet::new();
    for entry in &file_entries {
        if entry.file_type == FileType::File {
            if let Some(owner_name) = owners.get(&entry.path) {
                if owner_name.as_str() != partinfo.name {
                    warn!(
                        "[{}] diverted {} (owned by {})",
                        partinfo.name,
                        crate::util::compact_path(&entry.path),
                        owner_name
                    );
                    shadows.push((entry.path.clone(), owner_name.clone()));
                    divert_paths.insert(entry.path.clone());
                }
            }
        }
    }
    log_debug_timing(
        "install",
        &partinfo.name,
        "owner conflict check",
        phase_start.elapsed(),
    );

    let mut tx = TransactionContext::begin(
        db,
        TransactionOperation::Install,
        &partinfo.name,
        None,
        Some(&partinfo.plan.version),
    )
    .await?;

    let backup_dir = tempfile::tempdir()
        .map_err(|e| WrightError::InstallError(format!("failed to create backup dir: {}", e)))?;

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            log_running_hook(&partinfo.name, "pre_install");
            phase_start = Instant::now();
            if let Err(e) =
                run_install_script(script, root_dir, &partinfo.name, "pre_install").await
            {
                warn!("hook [pre_install] for {} failed: {}", partinfo.name, e);
            }
            log_debug_timing(
                "install",
                &partinfo.name,
                "pre_install hook",
                phase_start.elapsed(),
            );
        }
    }

    phase_start = Instant::now();
    match copy_entries_to_root(
        &file_entries,
        temp_dir.path(),
        root_dir,
        tx.rollback_state(),
        Some(backup_dir.path()),
        &HashSet::new(),
        &divert_paths,
    )
    .await
    {
        Ok(_) => {}
        Err(e) => {
            warn!("Installation failed, rolling back: {}", e);
            tx.rollback().await?;
            return Err(e);
        }
    }
    log_debug_timing(
        "install",
        &partinfo.name,
        "filesystem copy into target root",
        phase_start.elapsed(),
    );

    phase_start = Instant::now();
    let plan_id = db
        .ensure_plan_registered(
            &partinfo,
            &partinfo.plan.version,
            partinfo.plan.release,
            partinfo.plan.epoch,
            &partinfo.plan.arch,
        )
        .await?;
    let part_id = db
        .insert_part(NewPart {
            name: &partinfo.name,
            plan_id,
            part_hash: Some(part_hash.as_str()),
            install_scripts: hooks_content.as_deref(),
            origin,
        })
        .await?;

    for (path, owner_name) in shadows {
        if let Some(owner_part) = db.get_part(&owner_name).await? {
            let diverted_to = if divert_paths.contains(&path) {
                let mut p = PathBuf::from(&path);
                let mut os = p.file_name().unwrap().to_os_string();
                os.push(".wright-diverted");
                p.set_file_name(os);
                Some(p.to_string_lossy().to_string())
            } else {
                None
            };
            let _ = db
                .record_shadowed_file(&path, owner_part.id, part_id, diverted_to.as_deref())
                .await;
        }
    }

    db.insert_files(part_id, &file_entries).await?;

    let mut deps = Vec::new();
    for d in &partinfo.runtime_deps {
        let (name, constraint) = version::parse_dependency(d).unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency {
            name,
            version_constraint: constraint.map(|c| c.to_string()),
        });
    }

    if !deps.is_empty() {
        db.insert_dependencies(part_id, &deps).await?;
    }

    if !partinfo.conflicts.is_empty() {
        db.insert_conflicts(part_id, &partinfo.conflicts).await?;
    }
    if !partinfo.replaces.is_empty() {
        db.insert_replaces(part_id, &partinfo.replaces).await?;
    }

    tx.commit().await?;
    log_debug_timing(
        "install",
        &partinfo.name,
        "database update",
        phase_start.elapsed(),
    );

    if run_hooks {
        if let Some(ref script) = hooks.post_install {
            log_running_hook(&partinfo.name, "post_install");
            phase_start = Instant::now();
            if let Err(e) =
                run_install_script(script, root_dir, &partinfo.name, "post_install").await
            {
                warn!("hook [post_install] for {} failed: {}", partinfo.name, e);
            }
            log_debug_timing(
                "install",
                &partinfo.name,
                "post_install hook",
                phase_start.elapsed(),
            );
        }
    }

    let ver_rel = if partinfo.plan.version.is_empty() {
        format!("{}", partinfo.plan.release)
    } else {
        format!("{}-{}", partinfo.plan.version, partinfo.plan.release)
    };
    info!("Installed {}: {}", partinfo.name, ver_rel);

    log_debug_timing("install", &partinfo.name, "total", overall_start.elapsed());

    Ok(())
}
