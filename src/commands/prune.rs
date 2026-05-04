use crate::archive::prune;
use crate::cli::prune::PruneArgs;
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use anyhow::{Context, Result};

pub async fn execute_prune(args: PruneArgs, config: &GlobalConfig) -> Result<()> {
    prune_parts(config, args.latest, args.apply).await
}

async fn prune_parts(
    config: &GlobalConfig,
    keep_latest: bool,
    apply: bool,
) -> Result<()> {
    if !keep_latest {
        anyhow::bail!("nothing to do: pass --latest");
    }

    let parts_dir = &config.general.parts_dir;
    tokio::fs::create_dir_all(parts_dir)
        .await
        .with_context(|| format!("failed to create {}", parts_dir.display()))?;

    let installed_db = InstalledDb::open(&config.general.db_path)
        .await
        .context("failed to open installed-part database")?;

    let report = if apply {
        prune::apply_prune(&installed_db,
            parts_dir,
            keep_latest,
        )
        .await
        .context("prune failed")?
    } else {
        prune::plan_prune(&installed_db,
            parts_dir,
            keep_latest,
        )
        .await
        .context("prune planning failed")?
    };

    for stale in &report.stale {
        let ver_rel = if stale.version.is_empty() {
            format!("{}", stale.release)
        } else {
            format!("{}-{}", stale.version, stale.release)
        };
        println!(
            "prune: {} ({} {})",
            stale.path.display(),
            stale.name,
            ver_rel
        );
    }

    if report.stale.is_empty() {
        println!("nothing to prune");
        return Ok(());
    }

    if !apply {
        println!("dry-run only; rerun with --apply to delete the listed parts");
    }

    Ok(())
}
