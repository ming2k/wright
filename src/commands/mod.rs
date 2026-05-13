pub mod build;
pub mod maintenance;
pub mod query;
pub mod system;

use crate::cli::{Cli, Commands};
use crate::config::GlobalConfig;
use crate::error::Result;
use crate::part::store::LocalPartStore;
use std::path::{Path, PathBuf};

async fn crash_recover(db_path: &Path) {
    match crate::database::InstalledDb::open(db_path).await {
        Ok(db) => {
            let _ = crate::delivery::recover_if_needed(&db).await;
        }
        Err(_) => {}
    }
}

fn resolve_db(root: Option<&Path>, top_db: Option<PathBuf>, config: &GlobalConfig) -> PathBuf {
    top_db.unwrap_or_else(|| {
        if let Some(r) = root {
            if r != Path::new("/") {
                return r.join("var/lib/wright/wright.db");
            }
        }
        config.general.db_path.clone()
    })
}

/// Dispatch the parsed CLI command to the appropriate handler.
pub async fn dispatch(cli: Cli, config: &GlobalConfig) -> Result<()> {
    let top_db = cli.db.clone();
    let verbose = cli.verbose;
    let quiet = cli.quiet;

    match cli.command {
        // ── System Management ──────────────────────────────────────
        Commands::Merge(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            system::dispatch_merge(args, config, &db_path, &root_dir).await
        }
        Commands::Install(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            system::dispatch_install(args, config, &db_path, &root_dir, verbose, quiet).await
        }
        Commands::Upgrade(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            system::dispatch_upgrade(args, config, &db_path, &root_dir, verbose, quiet).await
        }
        Commands::Remove(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            system::dispatch_remove(args, config, &db_path, &root_dir).await
        }
        Commands::Assume(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            system::dispatch_assume(args, config, &db_path).await
        }
        Commands::Unassume(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            system::dispatch_unassume(args, config, &db_path).await
        }

        // ── Query & Inspection ─────────────────────────────────────
        Commands::List(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            query::dispatch_list(args, &db_path).await
        }
        Commands::Files(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            query::dispatch_files(args, &db_path).await
        }
        Commands::Check(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            query::dispatch_check(args, config, &db_path, &root_dir).await
        }
        Commands::History(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            query::dispatch_history(args, &db_path).await
        }
        Commands::Doctor(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            query::dispatch_doctor(args, config, &db_path, &root_dir).await
        }

        // ── Build & Packaging ──────────────────────────────────────
        Commands::Build(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            build::dispatch_build(args, config, &db_path, verbose, quiet).await
        }
        Commands::Lint(args) => build::dispatch_lint(args, config).await,
        Commands::Launch(mut args) => {
            let root_dir = args.root.take().unwrap_or_else(|| PathBuf::from("/"));
            let db_path = resolve_db(Some(&root_dir), top_db, config);
            crash_recover(&db_path).await;
            build::dispatch_launch(args, config, &db_path, &root_dir, verbose, quiet).await
        }

        // ── Cache & Maintenance ────────────────────────────────────
        Commands::Prune(args) => {
            let db_path = top_db.unwrap_or_else(|| config.general.db_path.clone());
            crash_recover(&db_path).await;
            maintenance::dispatch_prune(args, config, &db_path).await
        }
    }
}

/// Helper function to setup the local part store for commands that need it.
pub(crate) fn setup_local_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    crate::resolve::setup_part_store(config).map_err(Into::into)
}
