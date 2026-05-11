use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::part::prune;
use std::path::Path;

pub async fn execute_prune(
    config: &GlobalConfig,
    db_path: &Path,
    latest: bool,
    apply: bool,
) -> Result<()> {
    prune_parts(config, db_path, latest, apply).await
}

async fn prune_parts(
    config: &GlobalConfig,
    db_path: &Path,
    keep_latest: bool,
    apply: bool,
) -> Result<()> {
    if !keep_latest {
        return Err(WrightError::ForgeError(
            "nothing to do: pass --latest".into(),
        ));
    }

    let parts_dir = &config.general.parts_dir;
    tokio::fs::create_dir_all(parts_dir).await.map_err(|e| {
        WrightError::IoError(std::io::Error::other(format!(
            "failed to create {}: {}",
            parts_dir.display(),
            e
        )))
    })?;

    let installed_db = InstalledDb::open(db_path).await.map_err(|e| {
        WrightError::DatabaseError(format!("failed to open installed-part database: {}", e))
    })?;

    let report = if apply {
        prune::apply_prune(&installed_db, parts_dir, keep_latest)
            .await
            .map_err(|e| WrightError::ForgeError(format!("prune failed: {}", e)))?
    } else {
        prune::plan_prune(&installed_db, parts_dir, keep_latest)
            .await
            .map_err(|e| WrightError::ForgeError(format!("prune planning failed: {}", e)))?
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
