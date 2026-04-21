pub mod apply;
pub mod doctor;
pub mod install;
pub mod list;

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::archive::resolver::{pick_latest, pick_version};
use crate::cli::system::{Commands as SystemCommands, PrefixModeArg};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::query;
use crate::query::PrefixMode;
use crate::transaction;

pub async fn execute(
    command: SystemCommands,
    config: &GlobalConfig,
    installed_db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let _command_lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(installed_db_path),
        crate::util::lock::LockIdentity::Command("wright"),
        crate::util::lock::LockMode::Exclusive,
    )
    .context("failed to start wright operation")?;
    let resolver = crate::commands::setup_local_resolver(config)?;

    if let SystemCommands::Apply {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        force,
        dry_run,
    } = command
    {
        return apply::execute_apply(targets, deps, rdeps, match_policies, depth, force, dry_run, config, installed_db_path, root_dir, verbose, quiet, &resolver).await;
    }

    if let SystemCommands::SystemInit = command {
        println!("Initializing system databases...");
        
        println!("  -> {}...", installed_db_path.display());
        let _db = InstalledDb::open(installed_db_path).await.context("failed to initialize system database")?;
        
        let archive_db_path = &config.general.archive_db_path;
        println!("  -> {}...", archive_db_path.display());
        let _adb = crate::database::ArchiveDb::open(archive_db_path).await.context("failed to initialize archive database")?;
        
        println!("Databases are up-to-date.");
        return Ok(());
    }

    let db = InstalledDb::open(installed_db_path).await.context("failed to open database")?;

    match command {
        SystemCommands::Apply { .. } => unreachable!(),
        SystemCommands::SystemInit => unreachable!(),
        SystemCommands::Install { parts, force, nodeps } => { install::execute_install(&db, parts, force, nodeps, root_dir, &resolver).await?; }
        SystemCommands::List { long, roots, assumed, orphans } => { list::execute_list(&db, long, roots, assumed, orphans).await?; }
        SystemCommands::Doctor => { doctor::execute_doctor(&db).await?; }
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
                            eprintln!("error upgrading {}: {}", path.display(), e);
                            std::process::exit(1);
                        }
                    }
                    continue;
                }

                // Resolve by name
                let all_versions = resolver
                    .resolve_all(arg).await
                    .context(format!("failed to resolve '{}'", arg))?;

                if all_versions.is_empty() {
                    eprintln!("error: no parts found for '{}'", arg);
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
                        eprintln!(
                            "error: version '{}' not found for '{}'",
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
                ).await {
                    Ok(()) => println!(
                        "upgraded: {} -> {}-{}",
                        arg, selected.version, selected.release
                    ),
                    Err(e) => {
                        eprintln!("error upgrading {}: {}", arg, e);
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
            let batch_targets: HashSet<String> = if recursive {
                HashSet::new()
            } else {
                parts.iter().cloned().collect()
            };
            let removal_order = if recursive {
                parts.clone()
            } else {
                transaction::order_removal_batch(&db, &parts).await
                    .context("failed to plan removal order")?
            };

            for name in &removal_order {
                if recursive {
                    let dependents = db
                        .get_recursive_dependents(name).await
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
                                eprintln!("error removing {}: {}", dep, e);
                                std::process::exit(1);
                            }
                        }
                    }
                }

                // Compute cascade list before removing the target
                let cascade_list = if cascade {
                    let list = transaction::cascade_remove_list(&db, name).await
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
                    ).await
                };

                match result {
                    Ok(()) => println!("removed: {}", name),
                    Err(e) => {
                        eprintln!("error removing {}: {}", name, e);
                        std::process::exit(1);
                    }
                }

                // Remove orphan dependencies (leaf-first order)
                for orphan in &cascade_list {
                    match transaction::remove_part(&db, orphan, root_dir, true).await {
                        Ok(()) => println!("removed: {}", orphan),
                        Err(e) => {
                            eprintln!("error removing {}: {}", orphan, e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        SystemCommands::Deps {
            part,
            reverse,
            depth,
            filter,
            all,
            prefix: prefix_mode,
            prune,
        } => {
            use std::io::IsTerminal;
            let color = std::io::stdout().is_terminal();
            let mut buf = Vec::new();
            let prefix_mode = parse_prefix_mode(prefix_mode);

            let max_depth = if depth == 0 { usize::MAX } else { depth };
            let opts = query::TreeOptions {
                max_depth,
                filter: filter.as_deref(),
                prefix_mode,
                prune: &prune,
                color,
            };

            let stats = if all {
                if reverse {
                    writeln!(
                        buf,
                        "Installed reverse dependency tree for all parts (source: local part database):"
                    )?;
                } else {
                    writeln!(
                        buf,
                        "Installed dependency tree for all parts (source: local part database):"
                    )?;
                }
                writeln!(buf)?;
                query::write_system_tree(&db, &opts, &mut buf).await?
            } else {
                let part_name = part
                    .ok_or_else(|| anyhow::anyhow!("part name is required unless using --all"))?;

                let part = db.get_part(&part_name).await.context("failed to query part")?;
                if part.is_none() {
                    eprintln!("part '{}' is not installed", part_name);
                    std::process::exit(1);
                }

                if reverse {
                    writeln!(
                        buf,
                        "Installed reverse dependency tree for: {} (source: local part database)",
                        part_name
                    )?;
                } else {
                    writeln!(
                        buf,
                        "Installed dependency tree for: {} (source: local part database)",
                        part_name
                    )?;
                }
                writeln!(buf, "{}", part_name)?;
                if reverse {
                    query::write_reverse_dep_tree(&db, &part_name, &opts, &mut buf).await?
                } else {
                    query::write_dep_tree(&db, &part_name, &opts, &mut buf).await?
                }
            };

            stats.write_summary(&mut buf, color).ok();
            if color {
                writeln!(
                    buf,
                    "{}",
                    "Source: local part database (.PARTINFO-derived metadata)".dimmed()
                )
                .ok();
            } else {
                writeln!(
                    buf,
                    "Source: local part database (.PARTINFO-derived metadata)"
                )
                .ok();
            }
            print_paged(&String::from_utf8_lossy(&buf));
        }
        SystemCommands::Query { part } => {
            let installed_part = db.get_part(&part).await.context("failed to query part")?;
            match installed_part {
                Some(info) => {
                    println!("Name        : {}", info.name);
                    println!("Version     : {}", info.version);
                    println!("Release     : {}", info.release);
                    println!("Description : {}", info.description.unwrap_or_default());
                    println!("Architecture: {}", info.arch);
                    println!("License     : {}", info.license.unwrap_or_default());
                    if let Some(ref url) = info.url {
                        println!("URL         : {}", url);
                    }
                    println!("Install Size: {} bytes", info.install_size.unwrap_or(0));
                    println!("Origin      : {}", info.origin);
                    println!("Installed At: {}", info.installed_at.unwrap_or_default());
                    if let Some(ref hash) = info.part_hash {
                        println!("Part Hash   : {}", hash);
                    }
                    let opt_deps = db
                        .get_optional_dependencies(info.id).await
                        .context("failed to get optional dependencies")?;
                    if !opt_deps.is_empty() {
                        println!("Optional    :");
                        for name in &opt_deps {
                            println!("  {}", name);
                        }
                    }
                }
                None => {
                    eprintln!("part '{}' is not installed", part);
                    std::process::exit(1);
                }
            }
        }
        SystemCommands::Search { keyword } => {
            let results = db
                .search_parts(&keyword).await
                .context("failed to search parts")?;
            if results.is_empty() {
                println!("no parts found matching '{}'", keyword);
            } else {
                for installed_part in &results {
                    println!(
                        "{} {}-{} - {}",
                        installed_part.name,
                        installed_part.version,
                        installed_part.release,
                        installed_part.description.as_deref().unwrap_or_default()
                    );
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
                    eprintln!("part '{}' is not installed", part);
                    std::process::exit(1);
                }
            }
        }
        SystemCommands::Owner { file } => {
            match db.find_owner(&file).await.context("failed to find owner")? {
                Some(owner) => println!("{} is owned by {}", file, owner),
                None => {
                    println!("{} is not owned by any part", file);
                    std::process::exit(1);
                }
            }
        }
        SystemCommands::Verify { part } => {
            let parts_to_verify: Vec<String> = if let Some(name) = part {
                vec![name]
            } else {
                db.list_parts().await
                    .context("failed to list parts")?
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            };

            let mut all_ok = true;
            for name in &parts_to_verify {
                let issues = transaction::verify_part(&db, name, root_dir).await
                    .context(format!("failed to verify {}", name))?;
                if issues.is_empty() {
                    println!("{}: OK", name);
                } else {
                    all_ok = false;
                    println!("{}:", name);
                    for issue in &issues {
                        println!("  {}", issue);
                    }
                }
            }
            if !all_ok {
                std::process::exit(1);
            }
        }
        SystemCommands::Sysupgrade { dry_run } => {
            use crate::part::version::Version;

            let parts = db.list_parts().await.context("failed to list parts")?;
            let mut upgraded = 0usize;
            let mut up_to_date = 0usize;
            let mut not_found = 0usize;

            for part in &parts {
                match resolver.resolve_all(&part.name).await {
                    Ok(all_versions) if !all_versions.is_empty() => {
                        if let Some(latest) = pick_latest(&all_versions) {
                            let is_newer = {
                                let new_ver = Version::parse(&latest.version).ok();
                                let old_ver = Version::parse(&part.version).ok();
                                match (new_ver, old_ver) {
                                    (Some(nv), Some(ov)) => {
                                        if latest.epoch != part.epoch as u32 {
                                            latest.epoch > part.epoch as u32
                                        } else if nv != ov {
                                            nv > ov
                                        } else {
                                            latest.release > part.release as u32
                                        }
                                    }
                                    _ => {
                                        latest.version != part.version
                                            || latest.release > part.release as u32
                                    }
                                }
                            };

                            if is_newer {
                                println!(
                                    "upgrade: {} {}-{} -> {}-{}",
                                    part.name,
                                    part.version,
                                    part.release,
                                    latest.version,
                                    latest.release
                                );
                                if !dry_run {
                                    if let Err(e) = transaction::upgrade_part(
                                        &db,
                                        &latest.path,
                                        root_dir,
                                        false,
                                        true,
                                    ).await {
                                        eprintln!("  error: {}", e);
                                    } else {
                                        upgraded += 1;
                                    }
                                } else {
                                    upgraded += 1;
                                }
                            } else {
                                up_to_date += 1;
                            }
                        }
                    }
                    Ok(_) => {
                        not_found += 1;
                    }
                    Err(e) => eprintln!("warning: resolver error for {}: {}", part.name, e),
                }
            }

            if dry_run {
                println!(
                    "\n[dry-run] would upgrade {} part(s), {} up to date, {} not found",
                    upgraded, up_to_date, not_found
                );
            } else {
                println!(
                    "\nupgraded {}, {} up to date, {} not found",
                    upgraded, up_to_date, not_found
                );
            }
        }
        SystemCommands::Assume { name, version } => match db.assume_part(&name, &version).await {
            Ok(()) => println!("assumed: {} {}", name, version),
            Err(e) => {
                eprintln!("error: {:#}", e);
                std::process::exit(1);
            }
        },
        SystemCommands::Unassume { name } => match db.unassume_part(&name).await {
            Ok(()) => println!("unassumed: {}", name),
            Err(e) => {
                eprintln!("error: {:#}", e);
                std::process::exit(1);
            }
        },
        SystemCommands::Mark {
            parts,
            as_dependency,
            as_manual,
        } => {
            use crate::database::Origin;

            let origin = if as_dependency {
                Origin::Dependency
            } else if as_manual {
                Origin::Manual
            } else {
                eprintln!("error: specify --as-dependency or --as-manual");
                std::process::exit(1);
            };

            for name in &parts {
                match db.force_set_origin(name, origin).await {
                    Ok(()) => println!("{}: marked as {}", name, origin),
                    Err(e) => {
                        eprintln!("error: {}: {}", name, e);
                        std::process::exit(1);
                    }
                }
            }
        }
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
                    let status = if r.status != "completed" {
                        format!(" ({})", r.status)
                    } else {
                        String::new()
                    };
                    println!(
                        "{}  {:<9} {} {}{}",
                        r.timestamp.as_deref().unwrap_or_default(), r.operation, r.part_name, version, status
                    );
                }
            }
        }
    }
    Ok(())
}

fn print_paged(content: &str) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -R".to_string());
        let parts: Vec<&str> = pager.split_whitespace().collect();
        let (cmd, args) = parts.split_first().unwrap_or((&"less", &[][..]));
        if let Ok(mut child) = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(content.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
    print!("{}", content);
}

fn parse_prefix_mode(mode: PrefixModeArg) -> PrefixMode {
    match mode {
        PrefixModeArg::Indent => PrefixMode::Indent,
        PrefixModeArg::Depth => PrefixMode::Depth,
        PrefixModeArg::None => PrefixMode::None,
    }
}
