pub mod apply;
pub mod check;
pub mod doctor;
pub mod install;
pub mod list;

use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::cli::system::Commands as SystemCommands;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::part::store::{pick_latest, pick_version};
use crate::transaction;

pub async fn execute(
    command: SystemCommands,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let _command_lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(db_path),
        crate::util::lock::LockIdentity::Command("wright"),
        crate::util::lock::LockMode::Exclusive,
    )
    .context("failed to start wright operation")?;
    let part_store = crate::commands::setup_local_part_store(config)?;

    if let SystemCommands::Apply {
        targets,
        invalidate,
        deps,
        rdeps,
        match_policies,
        depth,
        force,
        dry_run,
    } = command
    {
        return apply::execute_apply(apply::ApplyArgs {
            targets,
            invalidate,
            deps,
            rdeps,
            match_policies,
            depth,
            force,
            dry_run,
            config,
            db_path,
            root_dir,
            verbose,
            quiet,
            part_store: &part_store,
        })
        .await;
    }

    if let SystemCommands::Install {
        parts,
        force,
        nodeps,
        invalidate,
        path,
    } = command
    {
        return install::execute_install(
            parts,
            force,
            nodeps,
            path,
            config,
            db_path,
            root_dir,
            &part_store,
            invalidate,
        )
        .await;
    }

    let db = InstalledDb::open(db_path)
        .await
        .context("failed to open database")?;

    match command {
        SystemCommands::Apply { .. } => unreachable!(),
        SystemCommands::Install { .. } => unreachable!(),
        SystemCommands::List {
            long,
            roots,
            assumed,
            orphans,
        } => {
            list::execute_list(&db, long, roots, assumed, orphans).await?;
        }
        SystemCommands::Check {
            part,
            deep,
            integrity_only,
        } => {
            check::execute_check(&db, root_dir, part.as_deref(), deep, integrity_only).await?;
        }
        SystemCommands::Upgrade {
            parts,
            force,
            version: target_version,
        } => {
            for arg in &parts {
                let path = PathBuf::from(arg);
                if path.exists() {
                    // Direct part file path
                    match transaction::upgrade_part(&db, &path, root_dir, force, true).await {
                        Ok(()) => println!("upgraded: {}", path.display()),
                        Err(e) => {
                            tracing::error!("upgrading {}: {}", path.display(), e);
                            std::process::exit(1);
                        }
                    }
                    continue;
                }

                // Resolve by name
                let all_versions = part_store
                    .resolve_all(arg)
                    .await
                    .context(format!("failed to resolve '{}'", arg))?;

                if all_versions.is_empty() {
                    tracing::error!("no parts found for '{}'", arg);
                    std::process::exit(1);
                }

                let selected = if let Some(ref ver) = target_version {
                    pick_version(&all_versions, ver)
                } else {
                    pick_latest(&all_versions)
                };

                let selected = match selected {
                    Some(s) => s,
                    None => {
                        tracing::error!(
                            "version '{}' not found for '{}'",
                            target_version.as_deref().unwrap_or("?"),
                            arg
                        );
                        std::process::exit(1);
                    }
                };

                // When --version is explicitly given, force the upgrade (allows downgrade)
                let effective_force = force || target_version.is_some();
                match transaction::upgrade_part(
                    &db,
                    &selected.path,
                    root_dir,
                    effective_force,
                    true,
                )
                .await
                {
                    Ok(()) => {
                        let ver_rel = if selected.version.is_empty() {
                            format!("{}", selected.release)
                        } else {
                            format!("{}-{}", selected.version, selected.release)
                        };
                        println!("upgraded: {} -> {}", arg, ver_rel);
                    }
                    Err(e) => {
                        tracing::error!("upgrading {}: {}", arg, e);
                        std::process::exit(1);
                    }
                }
            }
        }
        SystemCommands::Remove {
            parts,
            force,
            recursive,
            cascade,
        } => {
            // Check for assumed parts before attempting removal
            for name in &parts {
                if let Some(part) = db.get_part(name).await? {
                    if part.origin == crate::database::Origin::External {
                        tracing::error!("'{}' is externally provided. Use 'wright unassume {}' instead of 'remove'.", name, name);
                        std::process::exit(1);
                    }
                }
            }

            let batch_targets: HashSet<String> = if recursive {
                HashSet::new()
            } else {
                parts.iter().cloned().collect()
            };
            let removal_order = if recursive {
                parts.clone()
            } else {
                transaction::order_removal_batch(&db, &parts)
                    .await
                    .context("failed to plan removal order")?
            };

            for name in &removal_order {
                if recursive {
                    let dependents = db
                        .get_recursive_dependents(name)
                        .await
                        .context(format!("failed to resolve dependents of {}", name))?;

                    if !dependents.is_empty() {
                        println!(
                            "will also remove (depends on {}): {}",
                            name,
                            dependents.join(", ")
                        );
                    }

                    for dep in &dependents {
                        match transaction::remove_part(&db, dep, root_dir, true).await {
                            Ok(()) => println!("removed: {}", dep),
                            Err(e) => {
                                tracing::error!("removing {}: {}", dep, e);
                                std::process::exit(1);
                            }
                        }
                    }
                }

                // Compute cascade list before removing the target
                let cascade_list = if cascade {
                    let list = transaction::cascade_remove_list(&db, name)
                        .await
                        .context(format!("failed to compute cascade list for {}", name))?;
                    if !list.is_empty() {
                        println!(
                            "will also remove orphan dependencies of {}: {}",
                            name,
                            list.join(", ")
                        );
                    }
                    list
                } else {
                    Vec::new()
                };

                let result = if recursive {
                    transaction::remove_part(&db, name, root_dir, force || recursive).await
                } else {
                    let ignored_dependents: HashSet<String> = batch_targets
                        .iter()
                        .filter(|candidate| candidate.as_str() != name)
                        .cloned()
                        .collect();
                    transaction::remove_part_with_ignored_dependents(
                        &db,
                        name,
                        root_dir,
                        force,
                        &ignored_dependents,
                    )
                    .await
                };

                match result {
                    Ok(()) => println!("removed: {}", name),
                    Err(e) => {
                        tracing::error!("removing {}: {}", name, e);
                        std::process::exit(1);
                    }
                }

                // Remove orphan dependencies (leaf-first order)
                for orphan in &cascade_list {
                    match transaction::remove_part(&db, orphan, root_dir, true).await {
                        Ok(()) => println!("removed: {}", orphan),
                        Err(e) => {
                            tracing::error!("removing {}: {}", orphan, e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        SystemCommands::Files { part } => {
            let installed_part = db.get_part(&part).await.context("failed to query part")?;
            match installed_part {
                Some(info) => {
                    let files = db.get_files(info.id).await.context("failed to get files")?;
                    for file in &files {
                        println!("{}", file.path);
                    }
                }
                None => {
                    tracing::error!("part '{}' is not installed", part);
                    std::process::exit(1);
                }
            }
        }
        SystemCommands::Assume {
            name,
            version,
            file,
        } => {
            let mut entries: Vec<(String, String)> = Vec::new();

            if let Some(path) = file {
                let content = std::fs::read_to_string(&path)
                    .context(format!("failed to read {}", path.display()))?;
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    let mut parts = trimmed.split_whitespace();
                    let n = parts
                        .next()
                        .context(format!("missing name in line: {}", trimmed))?;
                    let v = parts
                        .next()
                        .context(format!("missing version in line: {}", trimmed))?;
                    entries.push((n.to_string(), v.to_string()));
                }
            } else if let (Some(n), Some(v)) = (name, version) {
                entries.push((n, v));
            } else if !std::io::stdin().is_terminal() {
                use std::io::BufRead;
                for line in std::io::stdin().lock().lines() {
                    let line = line.context("failed to read from stdin")?;
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    let mut parts = trimmed.split_whitespace();
                    let n = parts
                        .next()
                        .context(format!("missing name in line: {}", trimmed))?;
                    let v = parts
                        .next()
                        .context(format!("missing version in line: {}", trimmed))?;
                    entries.push((n.to_string(), v.to_string()));
                }
            } else {
                tracing::error!("provide name and version as arguments, use --file, or pipe input");
                std::process::exit(1);
            }

            for (n, v) in entries {
                match db.assume_part(&n, &v).await {
                    Ok(()) => println!("assumed: {} {}", n, v),
                    Err(e) => {
                        tracing::error!("assuming {}: {:#}", n, e);
                        std::process::exit(1);
                    }
                }
            }
        }
        SystemCommands::Unassume { name } => match db.unassume_part(&name).await {
            Ok(()) => println!("unassumed: {}", name),
            Err(e) => {
                tracing::error!("{:#}", e);
                std::process::exit(1);
            }
        },
        SystemCommands::History { part } => {
            let records = db.get_history(part.as_deref()).await?;
            if records.is_empty() {
                println!("no transaction history");
            } else {
                for r in &records {
                    let version = match (&r.old_version, &r.new_version) {
                        (None, Some(v)) => v.clone(),
                        (Some(v), None) => v.clone(),
                        (Some(old), Some(new)) => format!("{} -> {}", old, new),
                        (None, None) => String::new(),
                    };
                    let status = if r.status != crate::database::TransactionStatus::Completed {
                        format!(" ({})", r.status)
                    } else {
                        String::new()
                    };
                    println!(
                        "{}  {:<9} {} {}{}",
                        r.timestamp.as_deref().unwrap_or_default(),
                        r.operation,
                        r.part_name,
                        version,
                        status
                    );
                }
            }
        }
        SystemCommands::Doctor => {
            doctor::execute_doctor(&db, root_dir, config).await?;
        }
    }
    Ok(())
}
