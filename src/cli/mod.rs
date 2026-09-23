pub mod build;
pub mod check;
pub mod clean;
pub mod common;
pub mod doctor;
pub mod files;
pub mod graph;
pub mod history;
pub mod install;
pub mod launch;
pub mod lint;
pub mod list;
pub mod merge;
pub mod owner;
pub mod package;
pub mod plan;
pub mod provide;
pub mod remove;
pub mod resolve;
pub mod storage;
pub mod upgrade;

use clap::{ArgAction, Parser, Subcommand};
#[cfg(with_handlers)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::config::GlobalConfig;
#[cfg(with_handlers)]
use crate::error::Result;

#[cfg(with_handlers)]
use self::common::{Context, crash_recover, resolve_db};

#[derive(Parser)]
#[command(
    name = "wright",
    about = "Declarative, extensible, sandboxed Linux package manager",
    long_about = "Declarative, extensible, sandboxed Linux package manager",
    version,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Path to config file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub config: Option<PathBuf>,

    /// Path to database file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub db: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = ArgAction::Count, conflicts_with = "quiet", help_heading = "Global Options")]
    pub verbose: u8,

    /// Reduce log output (show warnings/errors only)
    #[arg(long, global = true, help_heading = "Global Options")]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(flatten, next_help_heading = "System Management")]
    System(SystemCommands),
    #[command(flatten, next_help_heading = "Query & Inspection")]
    Query(QueryCommands),
    #[command(flatten, next_help_heading = "Build & Packaging")]
    Build(BuildCommands),
    #[command(flatten, next_help_heading = "Cache & Maintenance")]
    Maintenance(MaintenanceCommands),
}

#[derive(Subcommand)]
pub enum SystemCommands {
    /// Converge system state from target plans (resolve -> build -> package -> merge)
    #[command(display_order = 1)]
    Install(install::InstallArgs),

    /// Rebuild and deploy plans with reverse dependency expansion
    #[command(display_order = 2)]
    Upgrade(upgrade::UpgradeArgs),

    /// Uninstall deployed parts (supports `plan`, `plan:*`, and `plan:output` targets)
    #[command(display_order = 3)]
    Remove(remove::RemoveArgs),

    /// Deploy pre-built archives directly into a target root
    #[command(display_order = 4)]
    Merge(merge::MergeArgs),

    /// Mark a part as externally provided to satisfy dependency checks
    #[command(display_order = 5)]
    Provide(provide::ProvideArgs),
}

#[derive(Subcommand)]
pub enum QueryCommands {
    /// List deployed plans and parts
    #[command(display_order = 10)]
    List(list::ListArgs),

    /// List files owned by deployed parts (supports `plan`, `plan:*`, and `plan:output` targets)
    #[command(display_order = 11)]
    Files(files::FilesArgs),

    /// Find which part owns a given path
    #[command(display_order = 12)]
    Owner(owner::OwnerArgs),

    /// Run system integrity and dependency checks
    #[command(display_order = 13)]
    Check(check::CheckArgs),

    /// Diagnose system, database, and archive health
    #[command(display_order = 14)]
    Doctor(doctor::DoctorArgs),

    /// Show transaction logs
    #[command(display_order = 15)]
    History(history::HistoryArgs),

    /// Print the plan source recorded when a plan's parts were sealed
    #[command(display_order = 16)]
    Plan(plan::PlanArgs),

    /// Show the global plan relationship graph (terminal or --web)
    #[command(display_order = 17)]
    Graph(graph::GraphArgs),
}

#[derive(Subcommand)]
pub enum BuildCommands {
    /// Compute the dependency execution graph for targets
    #[command(display_order = 20)]
    Resolve(resolve::ResolveArgs),

    /// Compile plan sources into sandboxed staging directories
    #[command(display_order = 21)]
    Build(build::BuildArgs),

    /// Seal built staging directories into `.wright.tar.zst` archives
    #[command(display_order = 22)]
    Package(package::PackageArgs),

    /// Fill a target root from a folio manifest or from plans
    #[command(display_order = 23)]
    Launch(launch::LaunchArgs),

    /// Verify plan syntax and logical integrity
    #[command(display_order = 24)]
    Lint(lint::LintArgs),
}

#[derive(Subcommand)]
pub enum MaintenanceCommands {
    /// Report disk usage for every location Wright owns, and how to reclaim it
    #[command(display_order = 29)]
    Storage(storage::StorageArgs),

    /// Reclaim disk space: build workspaces, part archives, and logs
    #[command(display_order = 30)]
    Clean(clean::CleanArgs),
}

/// Build a Context for a command that has a `--root` option.
///
/// `recover` runs crash recovery, which mutates the database. It is `true`
/// only for System-class commands; Read- and Local-class commands pass
/// `false` (ADR-0044, `[INV-PRIV-03]`).
#[cfg(with_handlers)]
async fn ctx_with_root<'a>(
    root: Option<PathBuf>,
    top_db: Option<PathBuf>,
    config: &'a GlobalConfig,
    verbose: u8,
    quiet: bool,
    recover: bool,
) -> Context<'a> {
    let root_dir = root.unwrap_or_else(|| PathBuf::from("/"));
    let db_path = resolve_db(Some(&root_dir), top_db, config);
    if recover {
        crash_recover(&db_path, config, &root_dir).await;
    }
    Context {
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
    }
}

/// Build a Context for a command that operates against the default root.
///
/// `recover` has the same meaning as in [`ctx_with_root`]: `true` only for
/// System-class commands.
#[cfg(with_handlers)]
async fn ctx_default<'a>(
    top_db: Option<PathBuf>,
    config: &'a GlobalConfig,
    verbose: u8,
    quiet: bool,
    recover: bool,
) -> Context<'a> {
    let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
    if recover {
        crash_recover(&db_path, config, Path::new("/")).await;
    }
    Context {
        config,
        db_path,
        root_dir: PathBuf::from("/"),
        verbose,
        quiet,
    }
}

#[cfg(with_handlers)]
pub async fn dispatch(cli: Cli, config: &GlobalConfig) -> Result<()> {
    let top_db = cli.db.clone();
    let verbose = cli.verbose;
    let quiet = cli.quiet;

    match cli.command {
        // ── System Management ───────────────────────────────────────
        Commands::System(SystemCommands::Install(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, true).await;
            install::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Upgrade(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, true).await;
            upgrade::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Remove(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, true).await;
            remove::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Merge(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, true).await;
            merge::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Provide(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, true).await;
            provide::run(args, &ctx).await
        }

        // ── Query & Inspection (Read class: no recovery, read-only DB) ──
        Commands::Query(QueryCommands::List(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, false).await;
            list::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Files(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, false).await;
            files::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Owner(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, false).await;
            owner::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Check(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, false).await;
            check::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Doctor(mut args)) => {
            let is_mutating = args.repair || args.restore.is_some() || args.snapshot.is_some();
            let ctx = ctx_with_root(
                args.root.take(),
                top_db,
                config,
                verbose,
                quiet,
                is_mutating,
            )
            .await;
            doctor::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::History(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, false).await;
            history::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Plan(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, false).await;
            plan::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Graph(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet, false).await;
            graph::run(args, &ctx).await
        }

        // ── Build & Packaging (Local class: no recovery) ────────────
        Commands::Build(BuildCommands::Resolve(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet, false).await;
            resolve::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Build(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet, false).await;
            build::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Package(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet, false).await;
            package::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Launch(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet, true).await;
            launch::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Lint(args)) => lint::run(args, config).await,

        // ── Cache & Maintenance ─────────────────────────────────────
        Commands::Maintenance(MaintenanceCommands::Storage(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet, false).await;
            storage::run(args, &ctx).await
        }
        Commands::Maintenance(MaintenanceCommands::Clean(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet, false).await;
            clean::run(args, &ctx).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands, MaintenanceCommands, SystemCommands};
    use clap::Parser;

    #[test]
    fn clean_command_parses_arguments() {
        let cli =
            Cli::try_parse_from(["wright", "clean", "hello", "--archives", "--logs"]).unwrap();
        let Commands::Maintenance(MaintenanceCommands::Clean(args)) = cli.command else {
            panic!("expected clean command");
        };
        assert_eq!(args.plans, vec!["hello"]);
        assert!(args.archives);
        assert!(!args.stale);
        assert!(args.logs);
        assert!(!args.dry_run);
    }

    #[test]
    fn clean_stale_conflicts_with_archives() {
        assert!(Cli::try_parse_from(["wright", "clean", "--stale", "--archives"]).is_err());
        let cli = Cli::try_parse_from(["wright", "clean", "--stale", "-n"]).unwrap();
        let Commands::Maintenance(MaintenanceCommands::Clean(args)) = cli.command else {
            panic!("expected clean command");
        };
        assert!(args.stale);
        assert!(args.dry_run);
    }

    #[test]
    fn clean_accepts_store_sources_and_ledger() {
        let cli = Cli::try_parse_from([
            "wright",
            "clean",
            "--store",
            "--sources",
            "--older-than-days",
            "30",
            "--ledger",
            "--keep-builds",
            "5",
            "--keep-snapshots",
            "3",
        ])
        .unwrap();
        let Commands::Maintenance(MaintenanceCommands::Clean(args)) = cli.command else {
            panic!("expected clean command");
        };
        assert!(args.store);
        assert!(args.sources);
        assert_eq!(args.older_than_days, Some(30));
        assert!(args.ledger);
        assert_eq!(args.keep_builds, 5);
        assert_eq!(args.keep_snapshots, 3);
    }

    #[test]
    fn clean_older_than_days_requires_sources() {
        assert!(Cli::try_parse_from(["wright", "clean", "--older-than-days", "7"]).is_err());
    }

    #[test]
    fn clean_keep_builds_requires_ledger() {
        assert!(Cli::try_parse_from(["wright", "clean", "--keep-builds", "5"]).is_err());
    }

    #[test]
    fn legacy_prune_command_is_gone() {
        // ADR-0040 promised removal after one release; ADR-0043 completes it.
        assert!(Cli::try_parse_from(["wright", "prune"]).is_err());
        assert!(Cli::try_parse_from(["wright", "prune", "--apply"]).is_err());
    }

    #[test]
    fn legacy_parts_alias_is_gone() {
        assert!(Cli::try_parse_from(["wright", "clean", "--parts"]).is_err());
    }

    #[test]
    fn legacy_clean_alias_is_gone() {
        assert!(Cli::try_parse_from(["wright", "install", "zlib", "--clean"]).is_err());
        assert!(Cli::try_parse_from(["wright", "upgrade", "all", "--clean"]).is_err());
    }

    #[test]
    fn storage_command_parses() {
        let cli = Cli::try_parse_from(["wright", "storage", "--json"]).unwrap();
        let Commands::Maintenance(MaintenanceCommands::Storage(args)) = cli.command else {
            panic!("expected storage command");
        };
        assert!(args.json);
    }

    #[test]
    fn legacy_usage_audit_db_commands_are_gone() {
        assert!(Cli::try_parse_from(["wright", "usage"]).is_err());
        assert!(Cli::try_parse_from(["wright", "audit"]).is_err());
        assert!(Cli::try_parse_from(["wright", "db"]).is_err());
        assert!(Cli::try_parse_from(["wright", "db", "backup"]).is_err());
    }

    #[test]
    fn doctor_flags_parse() {
        // Drift / audit
        let cli = Cli::try_parse_from(["wright", "doctor", "--drift", "--json"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Doctor(args)) = cli.command else {
            panic!("expected doctor");
        };
        assert!(args.drift);
        assert!(args.json);

        // Alias --audit for --drift
        let cli = Cli::try_parse_from(["wright", "doctor", "--audit"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Doctor(args)) = cli.command else {
            panic!("expected doctor");
        };
        assert!(args.drift);

        // Repair
        let cli =
            Cli::try_parse_from(["wright", "doctor", "--repair", "--from-store", "-n"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Doctor(args)) = cli.command else {
            panic!("expected doctor");
        };
        assert!(args.repair);
        assert!(args.from_store);
        assert!(args.dry_run);

        // Restore
        let cli = Cli::try_parse_from(["wright", "doctor", "--restore", "/tmp/x.bak"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Doctor(args)) = cli.command else {
            panic!("expected doctor");
        };
        assert_eq!(args.restore, Some(std::path::PathBuf::from("/tmp/x.bak")));

        // Snapshot
        let cli = Cli::try_parse_from(["wright", "doctor", "--snapshot", "/tmp/x.bak"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Doctor(args)) = cli.command else {
            panic!("expected doctor");
        };
        assert_eq!(
            args.snapshot,
            Some(Some(std::path::PathBuf::from("/tmp/x.bak")))
        );
    }

    #[test]
    fn graph_command_parses_arguments() {
        let cli = Cli::try_parse_from(["wright", "graph"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Graph(args)) = cli.command else {
            panic!("expected graph command");
        };
        assert!(!args.web);
        assert_eq!(args.port, 8642);
        assert!(!args.no_open);

        let cli =
            Cli::try_parse_from(["wright", "graph", "--web", "--port", "0", "--no-open"]).unwrap();
        let Commands::Query(crate::cli::QueryCommands::Graph(args)) = cli.command else {
            panic!("expected graph command");
        };
        assert!(args.web);
        assert_eq!(args.port, 0);
        assert!(args.no_open);

        // --port and --no-open only make sense with --web.
        assert!(Cli::try_parse_from(["wright", "graph", "--port", "9000"]).is_err());
        assert!(Cli::try_parse_from(["wright", "graph", "--no-open"]).is_err());
    }

    #[test]
    fn install_accepts_fresh() {
        let cli = Cli::try_parse_from(["wright", "install", "zlib", "--fresh"]).unwrap();

        let Commands::System(SystemCommands::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.fresh);
        assert!(!args.force);
    }

    #[test]
    fn upgrade_accepts_fresh() {
        let cli = Cli::try_parse_from(["wright", "upgrade", "all", "--fresh"]).unwrap();

        let Commands::System(SystemCommands::Upgrade(args)) = cli.command else {
            panic!("expected upgrade command");
        };
        assert!(args.fresh);
        assert!(!args.force);
    }

    #[test]
    fn launch_accepts_fresh() {
        let cli =
            Cli::try_parse_from(["wright", "launch", "--root", "/mnt/new", "--fresh"]).unwrap();

        let Commands::Build(crate::cli::BuildCommands::Launch(args)) = cli.command else {
            panic!("expected launch command");
        };
        assert!(args.fresh);
        assert!(!args.force);
    }
}
