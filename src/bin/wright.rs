use std::collections::HashSet;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use owo_colors::OwoColorize;
use tracing_subscriber::EnvFilter;

use wright::cli::wright::{Cli, Commands, PrefixModeArg};
use wright::config::GlobalConfig;
use wright::database::Database;
use wright::inventory::resolver::{pick_latest, pick_version, LocalResolver};
use wright::query;
use wright::query::PrefixMode;
use wright::transaction;

/// Write `content` to `$PAGER` (default: `less`) when stdout is a TTY,
/// otherwise print directly. Falls back to plain print if the pager fails.
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

fn setup_local_resolver(config: &GlobalConfig) -> Result<LocalResolver> {
    let mut resolver = wright::builder::orchestrator::setup_resolver(config)?;
    resolver.add_search_dir(config.general.components_dir.clone());
    if let Ok(cwd) = std::env::current_dir() {
        resolver.add_search_dir(cwd);
    }
    Ok(resolver)
}

fn collect_install_args(mut args: Vec<String>) -> Result<Vec<String>> {
    if !std::io::stdin().is_terminal() {
        for line in std::io::stdin().lock().lines() {
            let line = line.context("failed to read install target from stdin")?;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                args.push(trimmed.to_string());
            }
        }
    }
    Ok(args)
}

fn resolve_install_paths(resolver: &LocalResolver, args: &[String]) -> Result<Vec<PathBuf>> {
    let mut pkg_paths = Vec::new();
    for arg in args {
        let path = PathBuf::from(arg);
        if path.exists() {
            pkg_paths.push(path);
            continue;
        }

        match resolver.resolve(arg)? {
            Some(resolved) => pkg_paths.push(resolved.path),
            None => anyhow::bail!(
                "'{}' is not a file and could not be resolved from the local archive inventory",
                arg
            ),
        }
    }
    Ok(pkg_paths)
}

fn resolve_plan_targets(
    targets: &[String],
    resolver: &LocalResolver,
) -> Result<Vec<PathBuf>> {
    let all_plans = resolver.get_all_plans()?;
    let mut plans_to_build = HashSet::new();

    for target in targets {
        let clean_target = target.trim();
        if clean_target.is_empty() {
            continue;
        }

        if let Some(assembly_name) = clean_target.strip_prefix('@') {
            let paths = resolver.resolve_assembly(assembly_name)?;
            if paths.is_empty() {
                anyhow::bail!("assembly not found or empty: {}", assembly_name);
            }
            for path in paths {
                plans_to_build.insert(path);
            }
            continue;
        }

        if let Some(path) = all_plans.get(clean_target) {
            plans_to_build.insert(path.clone());
            continue;
        }

        let plan_path = PathBuf::from(clean_target);
        let manifest_path = if plan_path.is_file() {
            plan_path
        } else {
            plan_path.join("plan.toml")
        };

        if manifest_path.exists() {
            plans_to_build.insert(manifest_path);
            continue;
        }

        let mut found = false;
        for plans_dir in &resolver.plans_dirs {
            let candidate = plans_dir.join(clean_target).join("plan.toml");
            if candidate.exists() {
                wright::plan::manifest::PlanManifest::from_file(&candidate)
                    .context(format!("failed to parse plan '{}'", clean_target))?;
                plans_to_build.insert(candidate);
                found = true;
                break;
            }
        }

        if !found {
            anyhow::bail!("target not found: {}", clean_target);
        }
    }

    let mut ordered: Vec<_> = plans_to_build.into_iter().collect();
    ordered.sort();
    Ok(ordered)
}

fn archive_entries_for_plan(plan_path: &Path, archives_dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let manifest = wright::plan::manifest::PlanManifest::from_file(plan_path)?;
    let mut archives = vec![(
        manifest.plan.name.clone(),
        archives_dir.join(manifest.archive_filename()),
    )];
    if let Some(wright::plan::manifest::FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
        for (sub_name, sub_pkg) in pkgs {
            if sub_name == &manifest.plan.name {
                continue;
            }
            let sub_manifest = sub_pkg.to_manifest(sub_name, &manifest);
            archives.push((sub_name.clone(), archives_dir.join(sub_manifest.archive_filename())));
        }
    }
    Ok(archives)
}

fn apply_targets(
    config: &GlobalConfig,
    db: &Database,
    resolver: &LocalResolver,
    root_dir: &Path,
    targets: Vec<String>,
    force_build: bool,
    force_install: bool,
    nodeps: bool,
    verbose: bool,
    quiet: bool,
) -> Result<()> {
    let plan_paths = resolve_plan_targets(&targets, resolver)?;
    let mut explicit_plan_names = HashSet::new();
    for path in &plan_paths {
        let manifest = wright::plan::manifest::PlanManifest::from_file(path)
            .context(format!("failed to parse plan {}", path.display()))?;
        explicit_plan_names.insert(manifest.plan.name);
    }
    let build_set = wright::builder::orchestrator::resolve_build_set(
        config,
        targets.clone(),
        wright::builder::orchestrator::ResolveOptions {
            deps_mode: wright::builder::orchestrator::DependencyMode::Sync,
            dependents_mode: wright::builder::orchestrator::DependentsMode::None,
            depth: Some(0),
            include_self: true,
            install: true,
        },
    )?;

    let build_opts = wright::builder::orchestrator::BuildOptions {
        force: force_build,
        verbose,
        quiet,
        dockyards: config.build.dockyards,
        print_archives: false,
        nproc_per_dockyard: config.build.nproc_per_dockyard,
        ..Default::default()
    };
    let plan = wright::builder::orchestrator::create_execution_plan(
        config,
        build_set,
        &build_opts,
    )?;

    for (batch_idx, tasks) in plan.batches().iter().enumerate() {
        if !quiet {
            for task in tasks {
                tracing::info!(
                    "apply batch {} {}: {}",
                    batch_idx,
                    plan.label_for_task(task, &build_opts),
                    task.trim_end_matches(":bootstrap"),
                );
            }
        }

        plan.execute_batch(config, batch_idx, &build_opts)?;

        let mut archives = Vec::new();
        let mut explicit_targets = HashSet::new();
        let mut seen_paths = HashSet::new();
        for task in tasks {
            let plan_path = plan
                .plan_path_for_task(task)
                .context("missing plan path for batch task")?;
            for (part_name, archive_path) in
                archive_entries_for_plan(plan_path, &config.general.components_dir)?
            {
                if !archive_path.exists() {
                    anyhow::bail!(
                        "expected archive was not produced: {}",
                        archive_path.display()
                    );
                }
                if seen_paths.insert(archive_path.clone()) {
                    archives.push(archive_path);
                }
                if explicit_plan_names.contains(task.trim_end_matches(":bootstrap")) {
                    explicit_targets.insert(part_name);
                }
            }
        }

        transaction::install_parts_with_explicit_targets(
            db,
            &archives,
            &explicit_targets,
            root_dir,
            resolver,
            force_install,
            nodeps,
        )?;
    }

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose > 1 {
        EnvFilter::new("trace")
    } else if cli.verbose > 0 {
        EnvFilter::new("debug")
    } else if cli.quiet {
        EnvFilter::new("warn")
    } else {
        EnvFilter::new("info")
    };

    if cli.verbose > 0 {
        tracing_subscriber::fmt()
            .with_writer(wright::util::progress::MultiProgressWriter)
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(wright::util::progress::MultiProgressWriter)
            .without_time()
            .with_target(false)
            .with_level(true)
            .with_env_filter(filter)
            .init();
    }

    let config = GlobalConfig::load(cli.config.as_deref()).context("failed to load config")?;

    let db_path = cli.db.unwrap_or(config.general.db_path.clone());
    let root_dir = cli.root.unwrap_or_else(|| PathBuf::from("/"));
    let _command_lock = wright::util::lock::acquire_named_lock(&db_path, "wright")
        .context("failed to acquire wright command lock")?;

    let db = Database::open(&db_path).context("failed to open database")?;

    let resolver = setup_local_resolver(&config)?;

    match cli.command {
        Commands::Install {
            parts,
            force,
            nodeps,
        } => {
            let parts = collect_install_args(parts)?;
            let pkg_paths = resolve_install_paths(&resolver, &parts)?;

            match transaction::install_parts(&db, &pkg_paths, &root_dir, &resolver, force, nodeps)
            {
                Ok(()) => println!("installation completed successfully"),
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Apply {
            targets,
            force_build,
            force_install,
            nodeps,
        } => match apply_targets(
            &config,
            &db,
            &resolver,
            &root_dir,
            targets,
            force_build,
            force_install,
            nodeps,
            cli.verbose > 0,
            cli.quiet,
        ) {
            Ok(()) => println!("apply completed successfully"),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        },
        Commands::Upgrade {
            parts,
            force,
            version: target_version,
        } => {
            for arg in &parts {
                let path = PathBuf::from(arg);
                if path.exists() {
                    // Direct archive file path
                    match transaction::upgrade_part(&db, &path, &root_dir, force, true) {
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
                    .resolve_all(arg)
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
                    &root_dir,
                    effective_force,
                    true,
                ) {
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
        Commands::Remove {
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
                transaction::order_removal_batch(&db, &parts)
                    .context("failed to plan removal order")?
            };

            for name in &removal_order {
                if recursive {
                    let dependents = db
                        .get_recursive_dependents(name)
                        .context(format!("failed to resolve dependents of {}", name))?;

                    if !dependents.is_empty() {
                        println!(
                            "will also remove (depends on {}): {}",
                            name,
                            dependents.join(", ")
                        );
                    }

                    for dep in &dependents {
                        match transaction::remove_part(&db, dep, &root_dir, true) {
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
                    let list = transaction::cascade_remove_list(&db, name)
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
                    transaction::remove_part(&db, name, &root_dir, force || recursive)
                } else {
                    let ignored_dependents: HashSet<String> = batch_targets
                        .iter()
                        .filter(|candidate| candidate.as_str() != name)
                        .cloned()
                        .collect();
                    transaction::remove_part_with_ignored_dependents(
                        &db,
                        name,
                        &root_dir,
                        force,
                        &ignored_dependents,
                    )
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
                    match transaction::remove_part(&db, orphan, &root_dir, true) {
                        Ok(()) => println!("removed: {}", orphan),
                        Err(e) => {
                            eprintln!("error removing {}: {}", orphan, e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        Commands::Deps {
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
                query::write_system_tree(&db, &opts, &mut buf)?
            } else {
                let part_name = part
                    .ok_or_else(|| anyhow::anyhow!("part name is required unless using --all"))?;

                let pkg = db.get_part(&part_name).context("failed to query part")?;
                if pkg.is_none() {
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
                    query::write_reverse_dep_tree(&db, &part_name, &opts, &mut buf)?
                } else {
                    query::write_dep_tree(&db, &part_name, &opts, &mut buf)?
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
        Commands::List {
            long,
            roots,
            assumed,
            orphans,
        } => {
            let parts = if orphans {
                db.get_orphan_parts()
            } else if roots {
                db.get_root_parts()
            } else {
                db.list_parts()
            }
            .context("failed to list parts")?;

            if parts.is_empty() {
                if orphans {
                    println!("no orphan parts");
                } else {
                    println!("no parts installed");
                }
            } else {
                for pkg in &parts {
                    if assumed && !pkg.assumed {
                        continue;
                    }
                    if long {
                        if pkg.assumed {
                            println!("{:<12} {:<24} {}", "external", pkg.name, pkg.version);
                        } else {
                            println!(
                                "{:<12} {:<24} {}-{} ({})",
                                pkg.origin, pkg.name, pkg.version, pkg.release, pkg.arch
                            );
                        }
                    } else {
                        println!("{}", pkg.name);
                    }
                }
            }
        }
        Commands::Query { part } => {
            let installed_part = db.get_part(&part).context("failed to query part")?;
            match installed_part {
                Some(info) => {
                    println!("Name        : {}", info.name);
                    println!("Version     : {}", info.version);
                    println!("Release     : {}", info.release);
                    println!("Description : {}", info.description);
                    println!("Architecture: {}", info.arch);
                    println!("License     : {}", info.license);
                    if let Some(ref url) = info.url {
                        println!("URL         : {}", url);
                    }
                    println!("Install Size: {} bytes", info.install_size);
                    println!("Origin      : {}", info.origin);
                    println!("Installed At: {}", info.installed_at);
                    if let Some(ref hash) = info.pkg_hash {
                        println!("Part Hash   : {}", hash);
                    }
                    let opt_deps = db
                        .get_optional_dependencies(info.id)
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
        Commands::Search { keyword } => {
            let results = db
                .search_parts(&keyword)
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
                        installed_part.description
                    );
                }
            }
        }
        Commands::Files { part } => {
            let installed_part = db.get_part(&part).context("failed to query part")?;
            match installed_part {
                Some(info) => {
                    let files = db.get_files(info.id).context("failed to get files")?;
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
        Commands::Owner { file } => match db.find_owner(&file).context("failed to find owner")? {
            Some(owner) => println!("{} is owned by {}", file, owner),
            None => {
                println!("{} is not owned by any part", file);
                std::process::exit(1);
            }
        },
        Commands::Verify { part } => {
            let parts_to_verify: Vec<String> = if let Some(name) = part {
                vec![name]
            } else {
                db.list_parts()
                    .context("failed to list parts")?
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            };

            let mut all_ok = true;
            for name in &parts_to_verify {
                let issues = transaction::verify_part(&db, name, &root_dir)
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
        Commands::Sysupgrade { dry_run } => {
            use wright::part::version::Version;

            let parts = db.list_parts().context("failed to list parts")?;
            let mut upgraded = 0usize;
            let mut up_to_date = 0usize;
            let mut not_found = 0usize;

            for pkg in &parts {
                match resolver.resolve_all(&pkg.name) {
                    Ok(all_versions) if !all_versions.is_empty() => {
                        if let Some(latest) = pick_latest(&all_versions) {
                            let is_newer = {
                                let new_ver = Version::parse(&latest.version).ok();
                                let old_ver = Version::parse(&pkg.version).ok();
                                match (new_ver, old_ver) {
                                    (Some(nv), Some(ov)) => {
                                        if latest.epoch != pkg.epoch {
                                            latest.epoch > pkg.epoch
                                        } else if nv != ov {
                                            nv > ov
                                        } else {
                                            latest.release > pkg.release
                                        }
                                    }
                                    _ => {
                                        latest.version != pkg.version
                                            || latest.release > pkg.release
                                    }
                                }
                            };

                            if is_newer {
                                println!(
                                    "upgrade: {} {}-{} -> {}-{}",
                                    pkg.name,
                                    pkg.version,
                                    pkg.release,
                                    latest.version,
                                    latest.release
                                );
                                if !dry_run {
                                    if let Err(e) = transaction::upgrade_part(
                                        &db,
                                        &latest.path,
                                        &root_dir,
                                        false,
                                        true,
                                    ) {
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
                    Err(e) => eprintln!("warning: resolver error for {}: {}", pkg.name, e),
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
        Commands::Assume { name, version } => match db.assume_part(&name, &version) {
            Ok(()) => println!("assumed: {} {}", name, version),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        },
        Commands::Unassume { name } => match db.unassume_part(&name) {
            Ok(()) => println!("unassumed: {}", name),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        },
        Commands::Mark {
            parts,
            as_dependency,
            as_manual,
        } => {
            use wright::database::Origin;

            let origin = if as_dependency {
                Origin::Dependency
            } else if as_manual {
                Origin::Manual
            } else {
                eprintln!("error: specify --as-dependency or --as-manual");
                std::process::exit(1);
            };

            for name in &parts {
                match db.force_set_origin(name, origin) {
                    Ok(()) => println!("{}: marked as {}", name, origin),
                    Err(e) => {
                        eprintln!("error: {}: {}", name, e);
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::History { part } => {
            let records = db.get_history(part.as_deref())?;
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
                        r.timestamp, r.operation, r.part_name, version, status
                    );
                }
            }
        }
        Commands::Doctor => {
            println!("Wright System Health Report");
            println!("===========================");
            let mut total_issues = 0;

            // 1. Database Integrity
            print!("Checking database integrity... ");
            match db.integrity_check() {
                Ok(issues) if issues.is_empty() => println!("OK"),
                Ok(issues) => {
                    println!("FAILED");
                    for issue in issues {
                        println!("  [DB] {}", issue);
                    }
                    total_issues += 1;
                }
                Err(e) => {
                    println!("ERROR: {}", e);
                    total_issues += 1;
                }
            }

            // 2. Dependency Satisfaction
            print!("Checking dependency satisfaction... ");
            match query::check_dependencies(&db) {
                Ok(issues) if issues.is_empty() => println!("OK"),
                Ok(issues) => {
                    println!("FAILED");
                    for issue in issues {
                        println!("  [DEP] {}", issue);
                    }
                    total_issues += 1;
                }
                Err(e) => {
                    println!("ERROR: {}", e);
                    total_issues += 1;
                }
            }

            // 3. Circular Dependencies
            print!("Checking for circular dependencies... ");
            match query::check_circular_dependencies(&db) {
                Ok(issues) if issues.is_empty() => println!("OK"),
                Ok(issues) => {
                    println!("FAILED");
                    for issue in issues {
                        println!("  [CIRC] {}", issue);
                    }
                    total_issues += 1;
                }
                Err(e) => {
                    println!("ERROR: {}", e);
                    total_issues += 1;
                }
            }

            // 4. File Ownership
            print!("Checking for file ownership conflicts... ");
            match query::check_file_ownership_conflicts(&db) {
                Ok(issues) if issues.is_empty() => println!("OK"),
                Ok(issues) => {
                    println!("FAILED");
                    for issue in issues {
                        println!("  [FILE] {}", issue);
                    }
                    total_issues += 1;
                }
                Err(e) => {
                    println!("ERROR: {}", e);
                    total_issues += 1;
                }
            }

            // 5. Shadowed Files (History of Overwrites)
            print!("Checking for recorded file overlaps (shadows)... ");
            match query::check_shadowed_files(&db) {
                Ok(issues) if issues.is_empty() => println!("OK (None)"),
                Ok(issues) => {
                    println!("INFO (Found {} overlaps)", issues.len());
                    for issue in issues {
                        println!("  [SHADOW] {}", issue);
                    }
                    // We don't increment total_issues here as this is often intentional info
                }
                Err(e) => {
                    println!("ERROR: {}", e);
                    total_issues += 1;
                }
            }

            println!("===========================");
            if total_issues == 0 {
                println!("Result: System is healthy.");
            } else {
                println!("Result: Found {} categories of issues. Please fix them to ensure system stability.", total_issues);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
