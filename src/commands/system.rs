use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::system::{Commands as SystemCommands, PrefixModeArg};
use crate::config::GlobalConfig;
use crate::database::Database;
use crate::inventory::resolver::{pick_latest, pick_version, LocalResolver};
use crate::query;
use crate::query::PrefixMode;
use crate::transaction;

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

fn collect_install_args(mut args: Vec<String>) -> Result<Vec<String>> {
    use std::io::IsTerminal;
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
        if path.is_file() {
            pkg_paths.push(path);
            continue;
        }

        match resolver.resolve(arg)? {
            Some(resolved) => pkg_paths.push(resolved.path),
            None => anyhow::bail!(
                "'{}' is not a file and could not be resolved from the local part inventory",
                arg
            ),
        }
    }
    Ok(pkg_paths)
}

fn part_entries_for_plan(
    plan_path: &Path,
    parts_dir: &Path,
) -> Result<Vec<(String, PathBuf)>> {
    let manifest = crate::plan::manifest::PlanManifest::from_file(plan_path)?;
    let mut parts = vec![(
        manifest.plan.name.clone(),
        parts_dir.join(manifest.part_filename()),
    )];
    if let Some(crate::plan::manifest::FabricateConfig::Multi(ref pkgs)) = manifest.fabricate {
        for (sub_name, sub_pkg) in pkgs {
            if sub_name == &manifest.plan.name {
                continue;
            }
            let sub_manifest = sub_pkg.to_manifest(sub_name, &manifest);
            parts.push((
                sub_name.clone(),
                parts_dir.join(sub_manifest.part_filename()),
            ));
        }
    }
    Ok(parts)
}

struct ApplyContext<'a> {
    config: &'a GlobalConfig,
    db_path: &'a Path,
    resolver: &'a LocalResolver,
    root_dir: &'a Path,
    force_build: bool,
    force_install: bool,
    verbose: bool,
    quiet: bool,
    dry_run: bool,
}

fn apply_targets(
    ctx: ApplyContext,
    targets: Vec<String>,
    resolve_opts: crate::builder::orchestrator::ResolveOptions,
) -> Result<()> {
    use crate::builder::orchestrator::BuildExecutionPlan;
    use crate::builder::orchestrator::BuildOptions;

    // Single resolution pass to determine which targets were explicitly requested.
    // This drives install-origin tracking (explicit vs. dependency install).
    let explicit_plan_names =
        crate::builder::orchestrator::resolve_explicit_plan_names(ctx.resolver, &targets)?;

    let install_nodeps = resolve_opts.deps.is_none();

    let build_set = crate::builder::orchestrator::resolve_build_set(
        ctx.config,
        targets,
        resolve_opts,
    )?;

    let build_opts = BuildOptions {
        force: ctx.force_build,
        verbose: ctx.verbose,
        quiet: ctx.quiet,
        dockyards: ctx.config.build.dockyards,
        print_parts: false,
        nproc_per_dockyard: ctx.config.build.nproc_per_dockyard,
        ..Default::default()
    };
    let plan = crate::builder::orchestrator::create_execution_plan(ctx.config, build_set, &build_opts)?;

    if ctx.dry_run {
        println!("Apply plan (dry-run):");
        for (batch_idx, tasks) in plan.batches().iter().enumerate() {
            for task in tasks {
                let label = plan.label_for_task(task, &build_opts);
                let base = BuildExecutionPlan::task_base_name(task);
                let origin = if explicit_plan_names.contains(base) {
                    "explicit"
                } else {
                    "dep"
                };
                println!("  batch {batch_idx}  [{label}]  {base}  ({origin})");
            }
        }
        return Ok(());
    }

    // Track successfully installed parts so we can report partial-apply state on failure.
    let mut applied_parts: Vec<String> = Vec::new();

    for (batch_idx, tasks) in plan.batches().iter().enumerate() {
        if !ctx.quiet {
            for task in tasks {
                tracing::info!(
                    "Apply batch {} {}: {}",
                    batch_idx,
                    plan.label_for_task(task, &build_opts),
                    BuildExecutionPlan::task_base_name(task),
                );
            }
        }

        plan.execute_batch(ctx.config, batch_idx, &build_opts)
            .inspect_err(|_| {
                emit_partial_apply_note(&applied_parts);
            })?;

        let mut parts = Vec::new();
        let mut explicit_targets = HashSet::new();
        let mut batch_part_names = Vec::new();
        // Deduplicate part paths within a batch; multi-fabricate plans produce
        // multiple sub-parts from the same part file.
        let mut seen_paths = HashSet::new();

        for task in tasks {
            let base = BuildExecutionPlan::task_base_name(task);
            let plan_path = plan
                .plan_path_for_task(task)
                .context("missing plan path for batch task")?;
            for (part_name, part_path) in
                part_entries_for_plan(plan_path, &ctx.config.general.components_dir)?
            {
                if !part_path.exists() {
                    emit_partial_apply_note(&applied_parts);
                    anyhow::bail!(
                        "expected part was not produced: {}",
                        part_path.display()
                    );
                }
                if seen_paths.insert(part_path.clone()) {
                    parts.push(part_path);
                }
                if explicit_plan_names.contains(base) {
                    explicit_targets.insert(part_name.clone());
                }
                batch_part_names.push(part_name);
            }
        }

        let db = Database::open(ctx.db_path).context("failed to open database for install")?;

        transaction::install_parts_with_explicit_targets(
            &db,
            &parts,
            &explicit_targets,
            ctx.root_dir,
            ctx.resolver,
            ctx.force_install,
            install_nodeps,
        )
        .inspect_err(|_| {
            emit_partial_apply_note(&applied_parts);
        })?;

        applied_parts.extend(batch_part_names);
    }
    Ok(())
}

fn emit_partial_apply_note(applied_parts: &[String]) {
    if !applied_parts.is_empty() {
        eprintln!(
            "note: partially applied — already installed in previous batches: {}",
            applied_parts.join(", ")
        );
    }
}

pub fn execute(
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
    .context("failed to acquire wright command lock")?;
    let resolver = crate::commands::setup_local_resolver(config)?;

    // Apply is handled before opening the DB: it manages its own per-batch connections
    // (each batch opens and closes a handle for its install transaction).
    if let SystemCommands::Apply {
        targets,
        deps,
        rdeps,
        match_policies,
        depth,
        force_build,
        force_install,
        dry_run,
    } = command
    {
        use crate::builder::orchestrator::{MatchPolicy, DependentsMode, ResolveOptions};
        use crate::cli::resolve::{DomainArg, MatchPolicyArg};

        use std::io::IsTerminal;
        let targets = collect_install_args(targets)?;
        if targets.is_empty() {
            if !std::io::stdin().is_terminal() {
                anyhow::bail!("no targets received from stdin; did the resolve succeed?");
            }
            anyhow::bail!("no targets specified (pass plan names/paths as arguments or via stdin)");
        }

        let deps_domain = deps.map(|d| match d {
            DomainArg::Link => DependentsMode::Link,
            DomainArg::Runtime => DependentsMode::Runtime,
            DomainArg::Build => DependentsMode::Build,
            DomainArg::All => DependentsMode::All,
        });

        let rdeps_domain = rdeps.map(|d| match d {
            DomainArg::Link => DependentsMode::Link,
            DomainArg::Runtime => DependentsMode::Runtime,
            DomainArg::Build => DependentsMode::Build,
            DomainArg::All => DependentsMode::All,
        });

        let policies = match_policies
            .into_iter()
            .map(|p| match p {
                MatchPolicyArg::All => MatchPolicy::All,
                MatchPolicyArg::Missing => MatchPolicy::Missing,
                MatchPolicyArg::Outdated => MatchPolicy::Outdated,
                MatchPolicyArg::Installed => MatchPolicy::Installed,
            })
            .collect();

        let resolve_opts = ResolveOptions {
            deps: deps_domain,
            rdeps: rdeps_domain,
            match_policies: policies,
            depth,
            include_targets: true,
        };

        match apply_targets(
            ApplyContext {
                config,
                db_path,
                resolver: &resolver,
                root_dir,
                force_build,
                force_install,
                verbose: verbose > 0,
                quiet,
                dry_run,
            },
            targets,
            resolve_opts,
        ) {
            Ok(()) => {
                if !dry_run {
                    println!("apply completed successfully");
                }
            }
            Err(e) => {
                eprintln!("error: {:#}", e);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    let db = Database::open(db_path).context("failed to open database")?;

    match command {
        SystemCommands::Apply { .. } => unreachable!(),
        SystemCommands::Install {
            parts,
            force,
            nodeps,
        } => {
            use std::io::IsTerminal;
            let parts = collect_install_args(parts)?;
            if parts.is_empty() {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("no part paths received from stdin; did the build succeed?");
                }
                anyhow::bail!(
                    "no parts specified (pass part names/paths as arguments or via stdin)"
                );
            }
            let pkg_paths = resolve_install_paths(&resolver, &parts)?;

            match transaction::install_parts(&db, &pkg_paths, root_dir, &resolver, force, nodeps) {
                Ok(()) => println!("installation completed successfully"),
                Err(e) => {
                    eprintln!("error: {:#}", e);
                    std::process::exit(1);
                }
            }
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
                    match transaction::upgrade_part(&db, &path, root_dir, force, true) {
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
                    root_dir,
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
                        match transaction::remove_part(&db, dep, root_dir, true) {
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
                    transaction::remove_part(&db, name, root_dir, force || recursive)
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
                    match transaction::remove_part(&db, orphan, root_dir, true) {
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
        SystemCommands::List {
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
        SystemCommands::Query { part } => {
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
        SystemCommands::Search { keyword } => {
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
        SystemCommands::Files { part } => {
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
        SystemCommands::Owner { file } => {
            match db.find_owner(&file).context("failed to find owner")? {
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
                db.list_parts()
                    .context("failed to list parts")?
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            };

            let mut all_ok = true;
            for name in &parts_to_verify {
                let issues = transaction::verify_part(&db, name, root_dir)
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
                                        root_dir,
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
        SystemCommands::Assume { name, version } => match db.assume_part(&name, &version) {
            Ok(()) => println!("assumed: {} {}", name, version),
            Err(e) => {
                eprintln!("error: {:#}", e);
                std::process::exit(1);
            }
        },
        SystemCommands::Unassume { name } => match db.unassume_part(&name) {
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
                match db.force_set_origin(name, origin) {
                    Ok(()) => println!("{}: marked as {}", name, origin),
                    Err(e) => {
                        eprintln!("error: {}: {}", name, e);
                        std::process::exit(1);
                    }
                }
            }
        }
        SystemCommands::History { part } => {
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
        SystemCommands::Doctor => {
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
                println!(
                    "Result: Found {} categories of issues. Please fix them to ensure system stability.",
                    total_issues
                );
                std::process::exit(1);
            }
        }
    }

    Ok(())
}
