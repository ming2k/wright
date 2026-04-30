use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use std::time::Instant;

use tracing::{debug, info, warn};

use crate::archive::resolver::LocalResolver;
use crate::database::{DepType, Dependency, FileType, InstalledDb, NewPart, Origin};
use crate::error::{Result, WrightError};
use crate::part::part;
use crate::part::version::{self, Version};
use crate::transaction::fs::{collect_file_entries, copy_entries_to_root};
use crate::transaction::hooks::{log_running_hook, read_hooks, run_install_script};
use crate::transaction::rollback::RollbackState;

use super::{journal_path_from_db, log_debug_timing, remove_part, upgrade_part};

pub async fn install_parts(
    db: &InstalledDb,
    parts: &[PathBuf],
    root_dir: &Path,
    resolver: &LocalResolver,
    force: bool,
    nodeps: bool,
) -> Result<()> {
    let explicit_targets: HashSet<String> = parts
        .iter()
        .map(|path| resolver.read_part(path))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|resolved| resolved.name)
        .collect();
    install_parts_with_explicit_targets(
        db,
        parts,
        &explicit_targets,
        root_dir,
        resolver,
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
    resolver: &LocalResolver,
    force: bool,
    nodeps: bool,
) -> Result<()> {
    let mut resolved_map = HashMap::new();
    let mut targets = Vec::new();

    for path in parts {
        let resolved = resolver.read_part(path)?;
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

            let dependencies = if let Some(part) = resolved_map.get(&name) {
                part.dependencies.clone()
            } else {
                continue;
            };

            for dep in &dependencies {
                let dep = dep.trim();
                if dep.is_empty() {
                    return Err(WrightError::DependencyError(format!(
                        "part '{}' declares an empty dependency. Check its recipe file",
                        name
                    )));
                }

                let (dep_name, constraint) =
                    version::parse_dependency(dep).unwrap_or_else(|_| (dep.to_string(), None));

                #[allow(clippy::map_entry)]
                if !resolved_map.contains_key(&dep_name) {
                    if let Some(installed) = db.get_part(&dep_name).await? {
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

                    if !db.find_providers(&dep_name).await?.is_empty() {
                        continue;
                    }

                    if let Some(resolved) = resolver.resolve(&dep_name).await? {
                        queue.push(dep_name.clone());
                        resolved_map.insert(dep_name, resolved);
                    } else {
                        return Err(WrightError::DependencyError(format!(
                            "could not resolve dependency '{}' required by '{}'",
                            dep_name, name
                        )));
                    }
                } else {
                    queue.push(dep_name);
                }
            }
            processed.insert(name);
        }
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

    let staging_dir = std::path::Path::new("/var/lib/wright/staging");
    let _ = tokio::fs::create_dir_all(staging_dir).await;
    let temp_dir = tempfile::tempdir_in(staging_dir)
        .or_else(|_| tempfile::tempdir())
        .map_err(|e| WrightError::InstallError(format!("failed to create temp dir: {}", e)))?;

    let mut phase_start = Instant::now();
    let (partinfo, part_hash) = part::extract_part(part_path, temp_dir.path())?;
    log_debug_timing(
        "install",
        &partinfo.name,
        "archive extraction",
        phase_start.elapsed(),
    );

    for replaced_name in &partinfo.replaces {
        if db.get_part(replaced_name).await?.is_some() {
            info!("Replacing {} with {}", replaced_name, partinfo.name);
            remove_part(db, replaced_name, root_dir, true).await?;
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
            let providers = db.find_providers(conflict_name).await?;
            if !providers.is_empty() {
                return Err(WrightError::DependencyError(format!(
                    "part conflict detected: '{}' conflicts with '{}' (provided by {}). \
                     Please remove it first or use --force.",
                    partinfo.name,
                    conflict_name,
                    providers.join(", ")
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

        for prov in &partinfo.provides {
            let reverse = db.find_conflicting_parts(prov).await?;
            if !reverse.is_empty() {
                return Err(WrightError::DependencyError(format!(
                    "part conflict detected: installed part(s) {} conflict with '{}' (provided by '{}'). \
                     Please remove them first or use --force.",
                    reverse.join(", "),
                    prov,
                    partinfo.name
                )));
            }
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
                        super::compact_path(&entry.path),
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

    let tx_id = db
        .record_transaction(
            "install",
            &partinfo.name,
            None,
            Some(&partinfo.version),
            "pending",
            None,
        )
        .await?;

    let mut rollback_state = match journal_path_from_db(db) {
        Some(jp) => RollbackState::with_journal(jp),
        None => RollbackState::new(),
    };

    let backup_dir = tempfile::tempdir()
        .map_err(|e| WrightError::InstallError(format!("failed to create backup dir: {}", e)))?;

    if run_hooks {
        if let Some(ref script) = hooks.pre_install {
            log_running_hook(&partinfo.name, "pre_install");
            phase_start = Instant::now();
            if let Err(e) = run_install_script(script, root_dir).await {
                warn!("pre_install script failed: {}", e);
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
        &mut rollback_state,
        Some(backup_dir.path()),
        &HashSet::new(),
        &divert_paths,
    )
    .await
    {
        Ok(_) => {}
        Err(e) => {
            warn!("Installation failed, rolling back: {}", e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back").await?;
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
    let part_id = db
        .insert_part(NewPart {
            name: &partinfo.name,
            version: &partinfo.version,
            release: partinfo.release,
            epoch: partinfo.epoch,
            description: &partinfo.description,
            arch: &partinfo.arch,
            license: &partinfo.license,
            url: None,
            install_size: partinfo.install_size,
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
            dep_type: DepType::Runtime,
        });
    }

    if !deps.is_empty() {
        db.insert_dependencies(part_id, &deps).await?;
    }

    if !partinfo.optional_deps.is_empty() {
        db.insert_optional_dependencies(part_id, &partinfo.optional_deps)
            .await?;
    }

    if !partinfo.provides.is_empty() {
        db.insert_provides(part_id, &partinfo.provides).await?;
    }
    if !partinfo.conflicts.is_empty() {
        db.insert_conflicts(part_id, &partinfo.conflicts).await?;
    }
    if !partinfo.replaces.is_empty() {
        db.insert_replaces(part_id, &partinfo.replaces).await?;
    }

    db.update_transaction_status(tx_id, "completed").await?;
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
            if let Err(e) = run_install_script(script, root_dir).await {
                warn!("post_install script failed: {}", e);
            }
            log_debug_timing(
                "install",
                &partinfo.name,
                "post_install hook",
                phase_start.elapsed(),
            );
        }
    }

    rollback_state.commit();

    log_debug_timing("install", &partinfo.name, "total", overall_start.elapsed());
    info!(
        "Installed {}: {}-{}",
        partinfo.name, partinfo.version, partinfo.release
    );
    Ok(())
}
