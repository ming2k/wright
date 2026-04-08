use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::database::{Database, DepType, Dependency, FileType, NewPart, Origin};
use crate::error::{Result, WrightError};
use crate::part::archive;
use crate::part::version::{self, Version};
use crate::repo::source::{ResolvedPart, SimpleResolver};
use crate::transaction::fs::{collect_file_entries, copy_files_to_root};
use crate::transaction::hooks::{read_hooks, run_install_script};
use crate::transaction::rollback::RollbackState;

use super::{journal_path_from_db, remove_part, upgrade_part};

pub fn install_parts(
    db: &Database,
    archives: &[PathBuf],
    root_dir: &Path,
    resolver: &SimpleResolver,
    force: bool,
    nodeps: bool,
) -> Result<()> {
    let mut resolved_map = HashMap::new();
    let mut targets = Vec::new();

    for path in archives {
        let resolved = resolver.read_archive(path)?;
        targets.push(resolved.name.clone());
        resolved_map.insert(resolved.name.clone(), resolved);
    }

    if !nodeps {
        let mut queue = targets.clone();
        let mut processed = HashSet::new();

        while let Some(name) = queue.pop() {
            if processed.contains(&name) {
                continue;
            }

            let dependencies = if let Some(pkg) = resolved_map.get(&name) {
                pkg.dependencies.clone()
            } else {
                continue;
            };

            for dep in &dependencies {
                let (dep_name, constraint) =
                    version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None));

                #[allow(clippy::map_entry)]
                if !resolved_map.contains_key(&dep_name) {
                    if let Some(installed) = db.get_part(&dep_name)? {
                        if let Some(ref c) = constraint {
                            let installed_ver = Version::parse(&installed.version)?;
                            if !c.satisfies(&installed_ver) {
                                return Err(WrightError::DependencyError(format!(
                                    "installed {} {} does not satisfy constraint {}",
                                    dep_name, installed.version, c
                                )));
                            }
                        }
                        continue;
                    }

                    if !db.find_providers(&dep_name)?.is_empty() {
                        continue;
                    }

                    if let Some(resolved) = resolver.resolve(&dep_name)? {
                        queue.push(dep_name.clone());
                        resolved_map.insert(dep_name, resolved);
                    } else {
                        return Err(WrightError::DependencyError(format!(
                            "could not resolve dependency: {}",
                            dep_name
                        )));
                    }
                } else {
                    queue.push(dep_name);
                }
            }
            processed.insert(name);
        }
    }

    let mut sorted_names = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    for name in resolved_map.keys() {
        visit_resolved(
            name,
            &resolved_map,
            &mut visited,
            &mut visiting,
            &mut sorted_names,
        )?;
    }

    let target_set: HashSet<String> = targets.iter().cloned().collect();

    for name in sorted_names {
        let is_target = target_set.contains(&name);

        if db.get_part(&name)?.is_some() {
            if is_target {
                db.set_origin(&name, Origin::Manual)?;
            }
            if force {
                info!(plan = %name, "force reinstalling");
                let pkg = resolved_map.get(&name).expect("resolved package exists");
                upgrade_part(db, &pkg.path, root_dir, true, true)?;
            }
            continue;
        }

        let origin = if is_target {
            Origin::Manual
        } else {
            Origin::Dependency
        };
        let pkg = resolved_map.get(&name).expect("resolved package exists");
        info!(plan = %name, "installing (origin: {})", origin);
        install_part_with_origin(db, &pkg.path, root_dir, force, origin, true)?;
    }

    Ok(())
}

fn visit_resolved(
    name: &str,
    map: &HashMap<String, ResolvedPart>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    sorted: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(name) {
        return Ok(());
    }
    if visiting.contains(name) {
        return Err(WrightError::DependencyError(format!(
            "circular dependency: {}",
            name
        )));
    }

    visiting.insert(name.to_string());

    if let Some(pkg) = map.get(name) {
        for dep in &pkg.dependencies {
            let (dep_name, _) =
                version::parse_dependency(dep).unwrap_or_else(|_| (dep.clone(), None));
            if map.contains_key(&dep_name) {
                visit_resolved(&dep_name, map, visited, visiting, sorted)?;
            }
        }
    }

    visiting.remove(name);
    visited.insert(name.to_string());
    sorted.push(name.to_string());

    Ok(())
}

pub fn install_part(
    db: &Database,
    archive_path: &Path,
    root_dir: &Path,
    force: bool,
) -> Result<()> {
    install_part_with_origin(db, archive_path, root_dir, force, Origin::Manual, true)
}

pub fn install_part_with_origin(
    db: &Database,
    archive_path: &Path,
    root_dir: &Path,
    force: bool,
    origin: Origin,
    run_hooks: bool,
) -> Result<()> {
    // Prefer a staging dir on the same filesystem as root so that rename(2)
    // can be used instead of read+write copy during installation.
    let staging_base = archive_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| std::path::Path::new("/var/lib/wright"));
    let temp_dir = tempfile::Builder::new()
        .prefix("wright-stage-")
        .tempdir_in(staging_base)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::InstallError(format!("failed to create temp dir: {}", e)))?;

    let (pkginfo, pkg_hash) = archive::extract_archive(archive_path, temp_dir.path())?;

    for replaced_name in &pkginfo.replaces {
        if db.get_part(replaced_name)?.is_some() {
            info!(plan = %pkginfo.name, "replacing {}", replaced_name);
            remove_part(db, replaced_name, root_dir, true)?;
        }
    }

    if !force {
        for conflict_name in &pkginfo.conflicts {
            if db.get_part(conflict_name)?.is_some() {
                return Err(WrightError::DependencyError(format!(
                    "part conflict detected: '{}' conflicts with installed part '{}'. \
                     Please remove it first or use --force.",
                    pkginfo.name, conflict_name
                )));
            }
            let providers = db.find_providers(conflict_name)?;
            if !providers.is_empty() {
                return Err(WrightError::DependencyError(format!(
                    "part conflict detected: '{}' conflicts with '{}' (provided by {}). \
                     Please remove it first or use --force.",
                    pkginfo.name,
                    conflict_name,
                    providers.join(", ")
                )));
            }
        }

        let reverse_conflicts = db.find_conflicting_parts(&pkginfo.name)?;
        if !reverse_conflicts.is_empty() {
            return Err(WrightError::DependencyError(format!(
                "part conflict detected: installed part(s) {} conflict with '{}'. \
                 Please remove them first or use --force.",
                reverse_conflicts.join(", "),
                pkginfo.name
            )));
        }

        for prov in &pkginfo.provides {
            let reverse = db.find_conflicting_parts(prov)?;
            if !reverse.is_empty() {
                return Err(WrightError::DependencyError(format!(
                    "part conflict detected: installed part(s) {} conflict with '{}' (provided by '{}'). \
                     Please remove them first or use --force.",
                    reverse.join(", "),
                    prov,
                    pkginfo.name
                )));
            }
        }
    }

    if db.get_part(&pkginfo.name)?.is_some() {
        if force {
            debug!(
                "Part {} already installed, attempting upgrade/reinstall",
                pkginfo.name
            );
            return upgrade_part(db, archive_path, root_dir, true, run_hooks);
        }
        return Err(WrightError::PartAlreadyInstalled(pkginfo.name.clone()));
    }

    let (hooks_content, hooks) = read_hooks(temp_dir.path());
    let file_entries = collect_file_entries(temp_dir.path(), &pkginfo)?;

    info!(plan = %pkginfo.name, "installing: {} files", file_entries.len());

    let mut shadows = Vec::new();
    for entry in &file_entries {
        if entry.file_type == FileType::File {
            if let Some(owner_name) = db.find_owner(&entry.path)? {
                if force {
                    if owner_name != pkginfo.name {
                        warn!(
                            "{}: overwriting {} (owned by {})",
                            pkginfo.name, entry.path, owner_name
                        );
                        shadows.push((entry.path.clone(), owner_name));
                    }
                } else {
                    return Err(WrightError::FileConflict {
                        path: PathBuf::from(&entry.path),
                        owner: owner_name,
                    });
                }
            }
        }
    }

    let tx_id = db.record_transaction(
        "install",
        &pkginfo.name,
        None,
        Some(&pkginfo.version),
        "pending",
        None,
    )?;

    let mut rollback_state = match journal_path_from_db(db) {
        Some(jp) => RollbackState::with_journal(jp),
        None => RollbackState::new(),
    };

    let backup_dir = tempfile::tempdir()
        .map_err(|e| WrightError::InstallError(format!("failed to create backup dir: {}", e)))?;

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            info!(plan = %pkginfo.name, "running pre_install hook");
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("pre_install script failed: {}", e);
            }
        }
    }

    match copy_files_to_root(
        temp_dir.path(),
        root_dir,
        &mut rollback_state,
        Some(backup_dir.path()),
        &HashSet::new(),
    ) {
        Ok(_) => {}
        Err(e) => {
            warn!("Installation failed, rolling back: {}", e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back")?;
            return Err(e);
        }
    }

    let pkg_id = db.insert_part(NewPart {
        name: &pkginfo.name,
        version: &pkginfo.version,
        release: pkginfo.release,
        epoch: pkginfo.epoch,
        description: &pkginfo.description,
        arch: &pkginfo.arch,
        license: &pkginfo.license,
        url: None,
        install_size: pkginfo.install_size,
        pkg_hash: Some(pkg_hash.as_str()),
        install_scripts: hooks_content.as_deref(),
        origin,
    })?;

    for (path, owner_name) in shadows {
        if let Some(owner_pkg) = db.get_part(&owner_name)? {
            let _ = db.record_shadowed_file(&path, owner_pkg.id, pkg_id);
        }
    }

    db.insert_files(pkg_id, &file_entries)?;

    let mut deps = Vec::new();
    for d in &pkginfo.runtime_deps {
        let (name, constraint) = version::parse_dependency(d).unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency {
            name,
            constraint: constraint.map(|c| c.to_string()),
            dep_type: DepType::Runtime,
        });
    }

    if !deps.is_empty() {
        db.insert_dependencies(pkg_id, &deps)?;
    }

    if !pkginfo.optional_deps.is_empty() {
        db.insert_optional_dependencies(pkg_id, &pkginfo.optional_deps)?;
    }

    if !pkginfo.provides.is_empty() {
        db.insert_provides(pkg_id, &pkginfo.provides)?;
    }
    if !pkginfo.conflicts.is_empty() {
        db.insert_conflicts(pkg_id, &pkginfo.conflicts)?;
    }

    db.update_transaction_status(tx_id, "completed")?;

    if run_hooks {
        if let Some(ref script) = hooks.post_install {
            info!(plan = %pkginfo.name, "running post_install hook");
            if let Err(e) = run_install_script(script, root_dir) {
                warn!("post_install script failed: {}", e);
            }
        }
    }

    rollback_state.commit();

    info!(plan = %pkginfo.name, "installed {}-{}", pkginfo.version, pkginfo.release);
    Ok(())
}
