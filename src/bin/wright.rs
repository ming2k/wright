use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use wright::config::{GlobalConfig, KitsConfig};
use wright::database::Database;
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

#[derive(Parser)]
#[command(name = "wright", about = "wright system administrator", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Alternate root directory for file operations
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Path to database file
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Reduce log output (show warnings/errors only)
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Install packages from .wright.tar.zst files, package names, or @kits
    Install {
        /// Package files, package names, or @kit names
        #[arg(required = true)]
        packages: Vec<String>,

        /// Force reinstall even if already installed
        #[arg(long)]
        force: bool,

        /// Skip dependency resolution
        #[arg(long)]
        nodeps: bool,
    },
    /// Upgrade installed packages by name or from archive files
    Upgrade {
        /// Package names or archive files to upgrade
        #[arg(required = true)]
        packages: Vec<String>,

        /// Force upgrade even if version is not newer
        #[arg(long)]
        force: bool,

        /// Target a specific version (implies --force for downgrades)
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove installed packages
    Remove {
        /// Package names to remove
        #[arg(required = true)]
        packages: Vec<String>,

        /// Force removal even if other packages depend on this one
        #[arg(long)]
        force: bool,

        /// Recursively remove all packages that depend on the target
        #[arg(long, short)]
        recursive: bool,

        /// Also remove orphan dependencies (auto-installed deps no longer needed)
        #[arg(long, short = 'c')]
        cascade: bool,
    },
    /// Analyze installed package dependency relationships
    Deps {
        /// Package name
        package: Option<String>,

        /// Show reverse dependencies (what depends on this package)
        #[arg(long, short)]
        reverse: bool,

        /// Maximum depth to display (0 = unlimited)
        #[arg(long, short, default_value = "0")]
        depth: usize,

        /// Filter output to only show matching package names
        #[arg(long, short)]
        filter: Option<String>,

        /// Show dependency tree for all installed packages
        #[arg(long, short)]
        all: bool,

        /// Output prefix style: indent (tree), depth (flat + depth number), none (bare names)
        #[arg(long, default_value = "indent", value_parser = parse_prefix_mode)]
        prefix: PrefixMode,

        /// Hide the subtree of the named package (can be repeated)
        #[arg(long, action = clap::ArgAction::Append)]
        prune: Vec<String>,
    },
    /// List installed packages
    List {
        /// Show only top-level (root) packages with no installed dependents
        #[arg(long, short)]
        roots: bool,
        /// Show only assumed (externally provided) packages
        #[arg(long, short)]
        assumed: bool,
        /// Show only orphan packages (auto-installed deps no longer needed)
        #[arg(long, short)]
        orphans: bool,
    },
    /// Show detailed package information
    Query {
        /// Package name
        package: String,
    },
    /// Search installed packages by keyword
    Search {
        /// Search keyword
        keyword: String,
    },
    /// List files owned by a package
    Files {
        /// Package name
        package: String,
    },
    /// Find which package owns a file
    Owner {
        /// File path
        file: String,
    },
    /// Verify installed package file integrity (SHA-256 checksums)
    Verify {
        /// Package name; omit to verify all installed packages
        package: Option<String>,
    },
    /// Perform a full system health check (integrity, dependencies, file conflicts, shadows)
    Doctor,
    /// Mark a package as externally provided to satisfy dependency checks
    Assume {
        /// Package name
        name: String,
        /// Package version
        version: String,
    },
    /// Remove an assumed (externally provided) package record
    Unassume {
        /// Package name
        name: String,
    },
    /// Show package transaction history (install, upgrade, remove)
    History {
        /// Package name; omit to show all history
        package: Option<String>,
    },
    /// Upgrade all installed packages to latest available versions
    Sysupgrade {
        /// Preview what would be upgraded without actually doing it
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
}


fn parse_prefix_mode(s: &str) -> std::result::Result<PrefixMode, String> {
    match s {
        "indent" => Ok(PrefixMode::Indent),
        "depth" => Ok(PrefixMode::Depth),
        "none" => Ok(PrefixMode::None),
        _ => Err(format!(
            "invalid prefix mode '{}': expected indent, depth, or none",
            s
        )),
    }
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

    let repo_config =
        wright::config::RepoConfig::load(None).context("failed to load repo config")?;

    let db_path = cli.db.unwrap_or(config.general.db_path.clone());
    let root_dir = cli.root.unwrap_or_else(|| PathBuf::from("/"));

    let db = Database::open(&db_path).context("failed to open database")?;

    let mut resolver =
        wright::repo::source::SimpleResolver::new(config.general.cache_dir.join("packages"));
    resolver.load_from_config(&repo_config);
    resolver.add_search_dir(config.general.cache_dir.join("packages"));
    resolver.add_search_dir(config.general.components_dir.clone());
    resolver.add_search_dir(std::env::current_dir()?);
    resolver.add_plans_dir(config.general.plans_dir.clone());

    match cli.command {
        Commands::Install {
            packages,
            force,
            nodeps,
        } => {
            let kits = KitsConfig::load_all(&config.general.kits_dir)
                .context("failed to load kits config")?;

            // Expand @kit references and resolve package names to paths
            let mut pkg_paths: Vec<PathBuf> = Vec::new();
            for arg in &packages {
                if let Some(kit_name) = arg.strip_prefix('@') {
                    let members = kits.resolve(kit_name);
                    if members.is_empty() {
                        eprintln!("error: kit '{}' not found or empty", kit_name);
                        std::process::exit(1);
                    }
                    for name in &members {
                        match resolver.resolve(name) {
                            Ok(Some(resolved)) => pkg_paths.push(resolved.path),
                            Ok(None) => {
                                eprintln!(
                                    "error: package '{}' (from @{}) not found",
                                    name, kit_name
                                );
                                std::process::exit(1);
                            }
                            Err(e) => {
                                eprintln!("error: failed to resolve '{}': {}", name, e);
                                std::process::exit(1);
                            }
                        }
                    }
                } else {
                    let path = PathBuf::from(arg);
                    if path.exists() {
                        pkg_paths.push(path);
                    } else {
                        // Try resolving as a package name
                        match resolver.resolve(arg) {
                            Ok(Some(resolved)) => pkg_paths.push(resolved.path),
                            Ok(None) => {
                                eprintln!("error: '{}' is not a file and could not be resolved as a package name", arg);
                                std::process::exit(1);
                            }
                            Err(e) => {
                                eprintln!("error: failed to resolve '{}': {}", arg, e);
                                std::process::exit(1);
                            }
                        }
                    }
                }
            }

            match transaction::install_packages(
                &db, &pkg_paths, &root_dir, &resolver, force, nodeps,
            ) {
                Ok(()) => println!("installation completed successfully"),
                Err(e) => {
                    eprintln!("error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Upgrade {
            packages,
            force,
            version: target_version,
        } => {
            use wright::repo::source::{pick_latest, pick_version};

            for arg in &packages {
                let path = PathBuf::from(arg);
                if path.exists() {
                    // Direct archive file path
                    match transaction::upgrade_package(&db, &path, &root_dir, force) {
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
                match transaction::upgrade_package(&db, &selected.path, &root_dir, effective_force)
                {
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
            packages,
            force,
            recursive,
            cascade,
        } => {
            for name in &packages {
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
                        match transaction::remove_package(&db, dep, &root_dir, true) {
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

                match transaction::remove_package(&db, name, &root_dir, force || recursive) {
                    Ok(()) => println!("removed: {}", name),
                    Err(e) => {
                        eprintln!("error removing {}: {}", name, e);
                        std::process::exit(1);
                    }
                }

                // Remove orphan dependencies (leaf-first order)
                for orphan in &cascade_list {
                    match transaction::remove_package(&db, orphan, &root_dir, true) {
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
            package,
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

            let max_depth = if depth == 0 { usize::MAX } else { depth };
            let opts = query::TreeOptions {
                max_depth,
                filter: filter.as_deref(),
                prefix_mode,
                prune: &prune,
                color,
            };

            let stats = if all {
                query::write_system_tree(&db, &opts, &mut buf)?
            } else {
                let package_name = package.ok_or_else(|| {
                    anyhow::anyhow!("package name is required unless using --all")
                })?;

                let pkg = db
                    .get_package(&package_name)
                    .context("failed to query package")?;
                if pkg.is_none() {
                    eprintln!("package '{}' is not installed", package_name);
                    std::process::exit(1);
                }

                writeln!(buf, "{}", package_name)?;
                if reverse {
                    query::write_reverse_dep_tree(&db, &package_name, &opts, &mut buf)?
                } else {
                    query::write_dep_tree(&db, &package_name, &opts, &mut buf)?
                }
            };

            stats.write_summary(&mut buf, color).ok();
            print_paged(&String::from_utf8_lossy(&buf));
        }
        Commands::List {
            roots,
            assumed,
            orphans,
        } => {
            let packages = if orphans {
                db.get_orphan_packages()
            } else if roots {
                db.get_root_packages()
            } else {
                db.list_packages()
            }
            .context("failed to list packages")?;

            if packages.is_empty() {
                if orphans {
                    println!("no orphan packages");
                } else {
                    println!("no packages installed");
                }
            } else {
                for pkg in &packages {
                    if assumed && !pkg.assumed {
                        continue;
                    }
                    if pkg.assumed {
                        println!("{} {} [external]", pkg.name, pkg.version);
                    } else {
                        println!(
                            "{} {}-{} ({})",
                            pkg.name, pkg.version, pkg.release, pkg.arch
                        );
                    }
                }
            }
        }
        Commands::Query { package } => {
            let pkg = db
                .get_package(&package)
                .context("failed to query package")?;
            match pkg {
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
                    println!("Reason      : {}", info.install_reason);
                    println!("Installed At: {}", info.installed_at);
                    if let Some(ref hash) = info.pkg_hash {
                        println!("Package Hash: {}", hash);
                    }
                    let opt_deps = db
                        .get_optional_dependencies(info.id)
                        .context("failed to get optional dependencies")?;
                    if !opt_deps.is_empty() {
                        println!("Optional    :");
                        for (name, desc) in &opt_deps {
                            println!("  {} - {}", name, desc);
                        }
                    }
                }
                None => {
                    eprintln!("package '{}' is not installed", package);
                    std::process::exit(1);
                }
            }
        }
        Commands::Search { keyword } => {
            let results = db
                .search_packages(&keyword)
                .context("failed to search packages")?;
            if results.is_empty() {
                println!("no packages found matching '{}'", keyword);
            } else {
                for pkg in &results {
                    println!(
                        "{} {}-{} - {}",
                        pkg.name, pkg.version, pkg.release, pkg.description
                    );
                }
            }
        }
        Commands::Files { package } => {
            let pkg = db
                .get_package(&package)
                .context("failed to query package")?;
            match pkg {
                Some(info) => {
                    let files = db.get_files(info.id).context("failed to get files")?;
                    for file in &files {
                        println!("{}", file.path);
                    }
                }
                None => {
                    eprintln!("package '{}' is not installed", package);
                    std::process::exit(1);
                }
            }
        }
        Commands::Owner { file } => match db.find_owner(&file).context("failed to find owner")? {
            Some(owner) => println!("{} is owned by {}", file, owner),
            None => {
                println!("{} is not owned by any package", file);
                std::process::exit(1);
            }
        },
        Commands::Verify { package } => {
            let packages_to_verify: Vec<String> = if let Some(name) = package {
                vec![name]
            } else {
                db.list_packages()
                    .context("failed to list packages")?
                    .iter()
                    .map(|p| p.name.clone())
                    .collect()
            };

            let mut all_ok = true;
            for name in &packages_to_verify {
                let issues = transaction::verify_package(&db, name, &root_dir)
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
            use wright::repo::source::pick_latest;

            let packages = db.list_packages().context("failed to list packages")?;
            let mut upgraded = 0usize;
            let mut up_to_date = 0usize;
            let mut not_found = 0usize;

            for pkg in &packages {
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
                                    if let Err(e) = transaction::upgrade_package(
                                        &db,
                                        &latest.path,
                                        &root_dir,
                                        false,
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
        Commands::Assume { name, version } => match db.assume_package(&name, &version) {
            Ok(()) => println!("assumed: {} {}", name, version),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        },
        Commands::Unassume { name } => match db.unassume_package(&name) {
            Ok(()) => println!("unassumed: {}", name),
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        },
        Commands::History { package } => {
            let records = db.get_history(package.as_deref())?;
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
                        r.timestamp, r.operation, r.package_name, version, status
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
