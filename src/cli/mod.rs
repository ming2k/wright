pub mod build;
pub mod check;
pub mod common;
pub mod doctor;
pub mod files;
pub mod history;
pub mod install;
pub mod launch;
pub mod lint;
pub mod list;
pub mod merge;
pub mod provide;
pub mod remove;
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
    long_about = "Declarative, extensible, sandboxed Linux package manager\n\n\
                  Command groups:\n  \
                  System Management    install, remove, upgrade, merge, assume\n  \
                  Query & Inspection   list, files, check, history, doctor\n  \
                  Build & Packaging    build, lint, launch",
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
    #[arg(long, help_heading = "Global Options")]
    pub db: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = ArgAction::Count, help_heading = "Global Options")]
    pub verbose: u8,

    /// Reduce log output (show warnings/errors only)
    #[arg(long, global = true, help_heading = "Global Options")]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    // ── System Management ──────────────────────────────────────────
    /// Merge plan archives into the target root
    #[command(display_order = 1)]
    Merge(merge::MergeArgs),

    /// Install plans: resolve, forge, seal, and merge with full lifecycle
    #[command(display_order = 2)]
    Install(install::InstallArgs),

    /// Upgrade plans: resolve, rebuild, seal, and deploy with reverse dependency expansion
    #[command(display_order = 3)]
    Upgrade(upgrade::UpgradeArgs),

    /// Remove deployed parts
    #[command(display_order = 4)]
    Remove(remove::RemoveArgs),

    /// Mark a part as externally provided to satisfy dependency checks
    #[command(display_order = 5)]
    Provide(provide::ProvideArgs),

    // ── Query & Inspection ─────────────────────────────────────────
    /// List deployed parts
    #[command(display_order = 11)]
    List(list::ListArgs),

    /// List files owned by a part
    #[command(display_order = 12)]
    Files(files::FilesArgs),

    /// Perform system health checks
    #[command(display_order = 13)]
    Check(check::CheckArgs),

    /// Show part transaction history (deploy, upgrade, remove)
    #[command(display_order = 14)]
    History(history::HistoryArgs),

    /// Diagnose system and archive health issues
    #[command(display_order = 15)]
    Doctor(doctor::DoctorArgs),

    // ── Build & Packaging ──────────────────────────────────────────
    /// Forge parts from plans
    #[command(display_order = 21)]
    Build(build::BuildArgs),

    /// Verify the syntax and logical integrity of plan files
    #[command(display_order = 22)]
    Lint(lint::LintArgs),

    /// Fill a target root from a folio manifest or from plans
    #[command(display_order = 23)]
    Launch(launch::LaunchArgs),
}

/// Build a Context for a command that has a `--root` option.
/// The `root` argument is consumed from the command's args; `top_db` overrides
/// the default db path. crash_recover is run on the resulting db path.
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

/// Dispatch the parsed CLI command to the appropriate handler.
#[cfg(with_handlers)]
pub async fn dispatch(cli: Cli, config: &GlobalConfig) -> Result<()> {
    let top_db = cli.db.clone();
    let verbose = cli.verbose;
    let quiet = cli.quiet;

    match cli.command {
        // ── System Management ──────────────────────────────────────
        Commands::Merge(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            merge::run(args, &ctx).await
        }
        Commands::Install(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            install::run(args, &ctx).await
        }
        Commands::Upgrade(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            upgrade::run(args, &ctx).await
        }
        Commands::Remove(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            remove::run(args, &ctx).await
        }
        Commands::Provide(args) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            provide::run(args, &ctx).await
        }

        // ── Query & Inspection ─────────────────────────────────────
        Commands::List(args) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            list::run(args, &ctx).await
        }
        Commands::Files(args) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            files::run(args, &ctx).await
        }
        Commands::Check(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            check::run(args, &ctx).await
        }
        Commands::History(args) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            history::run(args, &ctx).await
        }
        Commands::Doctor(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            doctor::run(args, &ctx).await
        }

        // ── Build & Packaging ──────────────────────────────────────
        Commands::Build(args) => {
            let ctx = ctx_default(top_db, config, verbose, quiet).await;
            build::run(args, &ctx).await
        }
        Commands::Lint(args) => lint::run(args, config).await,
        Commands::Launch(mut args) => {
            let ctx = ctx_with_root(args.root.take(), top_db, config, verbose, quiet).await;
            launch::run(args, &ctx).await
        }
    }
}
