use anyhow::{Context, Result};
use crate::database::ArchiveDb;
use crate::archive::prune;
use crate::cli::prune::PruneArgs;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;

pub async fn execute_prune(args: PruneArgs, config: &GlobalConfig) -> Result<()> {
    prune_parts(config, args.untracked, args.latest, args.apply).await
}

async fn prune_parts(
    config: &GlobalConfig,
    prune_untracked: bool,
    keep_latest: bool,
    apply: bool,
) -> Result<()> {
    if !prune_untracked && !keep_latest {
        anyhow::bail!("nothing to do: pass --untracked and/or --latest");
    }

    let archive_db = ArchiveDb::open(&config.general.archive_db_path).await
        .context("failed to open local archive database")?;
    let parts_dir = &config.general.parts_dir;
    tokio::fs::create_dir_all(parts_dir).await
        .with_context(|| format!("failed to create {}", parts_dir.display()))?;

    let installed_db = InstalledDb::open(&config.general.installed_db_path).await
        .context("failed to open installed-part database")?;

    let report = if apply {
        prune::apply_prune(
            &archive_db,
            &installed_db,
            parts_dir,
            prune_untracked,
            keep_latest,
        ).await
        .context("prune failed")?
    } else {
        let stale_db_rows = archive_db
            .remove_missing_files(parts_dir).await
            .context("failed to reconcile missing archive files")?;
        let mut report = prune::plan_prune(
            &archive_db,
            &installed_db,
            parts_dir,
            prune_untracked,
            keep_latest,
        ).await
        .context("prune planning failed")?;
        report.stale_db_rows = stale_db_rows;
        report
    };

    for filename in &report.stale_db_rows {
        println!("inventory-stale: {}", filename);
    }
    for path in &report.untracked {
        println!("prune untracked: {}", path.display());
    }
    for stale in &report.stale_tracked {
        println!(
            "prune tracked: {} ({} {}-{})",
            stale.path.display(),
            stale.name,
            stale.version,
            stale.release
        );
    }

    if report.untracked.is_empty() && report.stale_tracked.is_empty() {
        println!("nothing to prune");
        return Ok(());
    }

    if !apply {
        println!("dry-run only; rerun with --apply to delete the listed parts");
    }

    Ok(())
}
