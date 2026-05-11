pub mod build;
pub mod maintenance;
pub mod query;
pub mod system;

use crate::cli::{Cli, Commands};
use crate::config::GlobalConfig;
use crate::error::Result;
use crate::part::store::LocalPartStore;
use std::path::PathBuf;

/// Dispatch the parsed CLI command to the appropriate handler.
pub async fn dispatch(
    cli: Cli,
    config: &GlobalConfig,
    db_path: PathBuf,
    root_dir: PathBuf,
) -> Result<()> {
    // Run crash recovery at startup — checks for incomplete delivery
    // transactions from a prior crash and cleans them up.
    // The DB is opened in a block scope so the exclusive lock is
    // released before the command handler opens it again.
    {
        match crate::database::InstalledDb::open(&db_path).await {
            Ok(db) => {
                let _ = crate::delivery::recover_if_needed(&db).await;
            }
            Err(_) => {
                // Database may not exist yet (first run).
            }
        }
    }

    match cli.command {
        // ── System Management ──────────────────────────────────────
        Commands::Merge(args) => system::dispatch_merge(args, config, &db_path, &root_dir).await,
        Commands::Install(args) => {
            system::dispatch_install(args, config, &db_path, &root_dir, cli.verbose, cli.quiet)
                .await
        }
        Commands::Upgrade(args) => {
            system::dispatch_upgrade(args, config, &db_path, &root_dir, cli.verbose, cli.quiet)
                .await
        }
        Commands::Remove(args) => system::dispatch_remove(args, config, &db_path, &root_dir).await,
        Commands::Assume(args) => system::dispatch_assume(args, config, &db_path).await,
        Commands::Unassume(args) => system::dispatch_unassume(args, config, &db_path).await,

        // ── Query & Inspection ─────────────────────────────────────
        Commands::List(args) => query::dispatch_list(args, &db_path).await,
        Commands::Files(args) => query::dispatch_files(args, &db_path).await,
        Commands::Check(args) => query::dispatch_check(args, config, &db_path, &root_dir).await,
        Commands::History(args) => query::dispatch_history(args, &db_path).await,
        Commands::Doctor(args) => query::dispatch_doctor(args, config, &db_path, &root_dir).await,

        // ── Build & Packaging ──────────────────────────────────────
        Commands::Build(args) => {
            build::dispatch_build(args, config, &db_path, cli.verbose, cli.quiet).await
        }
        Commands::Lint(args) => build::dispatch_lint(args, config).await,
        Commands::Launch(args) => {
            build::dispatch_launch(args, config, &db_path, &root_dir, cli.verbose, cli.quiet).await
        }

        // ── Cache & Maintenance ────────────────────────────────────
        Commands::Prune(args) => maintenance::dispatch_prune(args, config, &db_path).await,
    }
}

/// Helper function to setup the local part store for commands that need it.
pub(crate) fn setup_local_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    crate::resolve::setup_part_store(config).map_err(Into::into)
}
