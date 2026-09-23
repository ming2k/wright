use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_DOCTOR_AFTER_HELP: &str = "\
Examples:
  wright doctor
  wright doctor --drift
  wright doctor --drift --json
  wright doctor --repair
  wright doctor --repair --from-store -n
  wright doctor --restore /var/lib/wright/wright.db.pre-migrate-v19-to-v20.bak
  wright doctor --snapshot
  wright doctor --snapshot /backup/w.bak";

#[derive(Args)]
#[command(
    long_about = "Diagnose system health, detect file drift, and repair registry.\n\n\
                  By default, runs standard checks (integrity, files, dependencies, \
                  and archive closure).\n\n\
                  Flags:\n\
                    --drift      Walk the managed filesystem and report unowned drift\n\
                    --repair     Rebuild the registry index from archives in inventory\n\
                    --restore    Replace the registry with a previously written snapshot\n\
                    --snapshot   Write a consistent standalone snapshot of the registry",
    after_help = WRIGHT_DOCTOR_AFTER_HELP
)]
pub struct DoctorArgs {
    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Check for unowned file drift on the live root
    #[arg(long, visible_alias = "audit")]
    pub drift: bool,

    /// Emit a machine-readable JSON report (supported with --drift, --snapshot, or --repair)
    #[arg(long)]
    pub json: bool,

    /// Include directories when checking drift
    #[arg(long, requires = "drift")]
    pub include_dirs: bool,

    /// Repair the registry index from part archives in inventory
    #[arg(long, conflicts_with_all = ["drift", "restore", "snapshot"])]
    pub repair: bool,

    /// Restore the registry from a snapshot file
    #[arg(long, value_name = "BACKUP", conflicts_with_all = ["drift", "repair", "snapshot"])]
    pub restore: Option<PathBuf>,

    /// Save a consistent snapshot of the registry to a file (default: <db>.backup)
    #[arg(long, value_name = "PATH", num_args = 0..=1, conflicts_with_all = ["drift", "repair", "restore"])]
    pub snapshot: Option<Option<PathBuf>>,

    /// Include orphaned copies in CAS store during repair
    #[arg(long, requires = "repair")]
    pub from_store: bool,

    /// Preview actions without writing changes (supported with --repair)
    #[arg(short = 'n', long)]
    pub dry_run: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: DoctorArgs, ctx: &Context<'_>) -> Result<()> {
    let root_dir = args.root.clone().unwrap_or_else(|| ctx.root_dir.clone());

    if let Some(backup) = args.restore {
        let file_name = ctx
            .db_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wright.db");
        let _lock = crate::util::lock::acquire_lock(
            &crate::util::lock::lock_dir_from_db(&ctx.db_path),
            crate::util::lock::LockIdentity::Database(file_name),
            crate::util::lock::LockMode::Exclusive,
        )
        .map_err(|e| crate::error::WrightError::context("failed to lock database", e))?;

        return crate::operations::dbadmin::execute_restore(&backup, &ctx.db_path);
    }

    if let Some(output) = args.snapshot {
        let db = ctx.open_db().await?;
        return crate::operations::dbadmin::execute_backup(
            &db,
            &ctx.db_path,
            output.as_deref(),
            args.json,
        )
        .await;
    }

    if args.repair {
        let db = ctx.open_db().await?;
        return crate::operations::dbadmin::execute_reindex(
            ctx.config,
            &root_dir,
            &db,
            args.from_store,
            args.dry_run,
            args.json,
        )
        .await;
    }

    if args.drift {
        let db = ctx.open_read_only().await?;
        return crate::operations::audit::execute_audit(
            &db,
            &root_dir,
            args.json,
            args.include_dirs,
        )
        .await;
    }

    match ctx.open_read_only().await {
        Ok(db) => crate::operations::doctor::execute_doctor(&db, &ctx.root_dir, ctx.config).await,
        Err(e) => {
            // A damaged registry must not take `doctor` down with it.
            let reason = crate::util::logging::flatten_error_causes(&e);
            crate::operations::doctor::execute_doctor_degraded(ctx.config, &reason).await
        }
    }
}
