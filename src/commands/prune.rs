use anyhow::{Context, Result};

use crate::cli::prune::PruneArgs;
use crate::config::GlobalConfig;
use crate::database::Database;
use crate::inventory::db::InventoryDb;
use crate::inventory::prune;

pub fn execute_prune(args: PruneArgs, config: &GlobalConfig) -> Result<()> {
    prune_parts(config, args.untracked, args.latest, args.apply)
}

fn prune_parts(
    config: &GlobalConfig,
    prune_untracked: bool,
    keep_latest: bool,
    apply: bool,
) -> Result<()> {
    if !prune_untracked && !keep_latest {
        anyhow::bail!("nothing to do: pass --untracked and/or --latest");
    }

    let inventory = InventoryDb::open(&config.general.inventory_db_path)
        .context("failed to open local inventory database")?;
    let parts_dir = &config.general.parts_dir;
    std::fs::create_dir_all(parts_dir)
        .with_context(|| format!("failed to create {}", parts_dir.display()))?;

    let installed_db = Database::open(&config.general.db_path)
        .context("failed to open installed-part database")?;

    let report = if apply {
        prune::apply_prune(
            &inventory,
            &installed_db,
            parts_dir,
            prune_untracked,
            keep_latest,
        )
        .context("prune failed")?
    } else {
        let stale_db_rows = inventory
            .remove_missing_files(parts_dir)
            .context("failed to reconcile missing archive files")?;
        let mut report = prune::plan_prune(
            &inventory,
            &installed_db,
            parts_dir,
            prune_untracked,
            keep_latest,
        )
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
