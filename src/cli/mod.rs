pub mod build;
pub mod check;
pub mod clean;
pub mod common;
pub mod doctor;
pub mod files;
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
pub mod prune;
pub mod remove;
pub mod resolve;
pub mod upgrade;

use clap::{ArgAction, Parser, Subcommand};
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
    /// Clean plan build workspaces, staging trees, and logs
    #[command(display_order = 30)]
    Clean(clean::CleanArgs),

    /// Remove obsolete local part archives
    #[command(display_order = 31)]
    Prune(prune::PruneArgs),
}

/// Build a Context for a command that has a `--root` option.
/// The `root` argument is consumed from the command's args; `top_db` overrides
/// the default db path. Crash recovery runs against the resulting db path.
#[cfg(with_handlers)]
async fn ctx_with_root<'a>(
    root: Option<PathBuf>,
    top_db: Option<PathBuf>,
    config: &'a GlobalConfig,
    verbose: u8,
    quiet: bool,
) -> Context<'a> {
    let root_dir = root.unwrap_or_else(|| PathBuf::from("/"));
    let db_path = resolve_db(Some(&root_dir), top_db, config);
    crash_recover(&db_path).await;
    Context {
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
    }
}

/// Build a Context for a command that operates against the default root.
#[cfg(with_handlers)]
async fn ctx_default<'a>(
    top_db: Option<PathBuf>,
    config: &'a GlobalConfig,
    verbose: u8,
    quiet: bool,
) -> Context<'a> {
    let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
    crash_recover(&db_path).await;
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
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            install::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Upgrade(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            upgrade::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Remove(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            remove::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Merge(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            merge::run(args, &ctx).await
        }
        Commands::System(SystemCommands::Provide(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            provide::run(args, &ctx).await
        }

        // ── Query & Inspection ─────────────────────────────────────
        Commands::Query(QueryCommands::List(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            list::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Files(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            files::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Owner(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            owner::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Check(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            check::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Doctor(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            doctor::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::History(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            history::run(args, &ctx).await
        }
        Commands::Query(QueryCommands::Plan(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            plan::run(args, &ctx).await
        }

        // ── Build & Packaging ───────────────────────────────────────
        Commands::Build(BuildCommands::Resolve(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            resolve::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Build(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            build::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Package(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            package::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Launch(mut args)) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            launch::run(args, &ctx).await
        }
        Commands::Build(BuildCommands::Lint(args)) => lint::run(args, config).await,

        // ── Cache & Maintenance ─────────────────────────────────────
        Commands::Maintenance(MaintenanceCommands::Clean(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            clean::run(args, &ctx).await
        }
        Commands::Maintenance(MaintenanceCommands::Prune(args)) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            prune::run(args, &ctx).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands, MaintenanceCommands, SystemCommands};
    use clap::Parser;

    #[test]
    fn clean_command_parses_arguments() {
        let cli = Cli::try_parse_from(["wright", "clean", "hello", "--parts", "--logs"]).unwrap();
        let Commands::Maintenance(MaintenanceCommands::Clean(args)) = cli.command else {
            panic!("expected clean command");
        };
        assert_eq!(args.plans, vec!["hello"]);
        assert!(args.parts);
        assert!(args.logs);
    }

    #[test]
    fn install_accepts_clean() {
        let cli = Cli::try_parse_from(["wright", "install", "zlib", "--clean"]).unwrap();

        let Commands::System(SystemCommands::Install(args)) = cli.command else {
            panic!("expected install command");
        };
        assert!(args.clean);
        assert!(!args.force);
    }
}
