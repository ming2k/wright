use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_part::archive;
use wright_part::store::ResolvedPartVersioned;

pub async fn execute_prune(apply: bool, config: &GlobalConfig) -> Result<()> {
    let parts_dir = &config.general.parts_dir;
    if !parts_dir.exists() {
        crate::cli_output!("No parts directory found at {}.", parts_dir.display());
        return Ok(());
    }

    let mut parts_by_name: HashMap<(String, String), Vec<ResolvedPartVersioned>> = HashMap::new();
    if let Ok(entries) = std::fs::read_dir(parts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".wright.tar.zst"))
                && let Ok(info) = archive::read_partinfo(&path)
            {
                let versioned = ResolvedPartVersioned {
                    name: info.name.clone(),
                    plan_name: info.plan.name.clone(),
                    version: info.plan.version,
                    release: info.plan.release,
                    epoch: info.plan.epoch,
                    path: path.clone(),
                    dependencies: info.runtime_deps,
                };
                // Group by (plan, part): unrelated plans may ship same-named
                // parts, and a newer version of one must not obsolete the
                // other's archive.
                parts_by_name
                    .entry((info.plan.name, info.name))
                    .or_default()
                    .push(versioned);
            }
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    for mut archives in parts_by_name.into_values() {
        if archives.len() > 1 {
            archives.sort_by(|left, right| right.version_cmp(left));
            for archive in archives.iter().skip(1) {
                candidates.push(archive.path.clone());
            }
        }
    }
    candidates.sort();

    if candidates.is_empty() {
        crate::cli_output!("No obsolete parts to prune.");
        return Ok(());
    }

    if !apply {
        crate::cli_action!(
            "Dry-run",
            "{} older archive(s) can be removed (pass --apply to execute)",
            candidates.len()
        );
        for p in &candidates {
            crate::cli_output!("  {}", p.display());
        }
        return Ok(());
    }

    crate::cli_action!("Pruning", "{} older archive(s)", candidates.len());
    for p in &candidates {
        std::fs::remove_file(p).map_err(|error| {
            WrightError::IoError(std::io::Error::new(
                error.kind(),
                format!("failed to remove {}: {error}", p.display()),
            ))
        })?;
    }

    crate::cli_output!("Pruned {} archive(s).", candidates.len());
    Ok(())
}
