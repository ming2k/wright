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
    // Recurse: current archives live in per-plan subdirectories, older
    // ones flat at the top level. Identity comes from .PARTINFO either way.
    for entry in walkdir::WalkDir::new(parts_dir).into_iter().flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".wright.tar.zst"))
            && let Ok(info) = archive::read_partinfo(path)
        {
            let versioned = ResolvedPartVersioned {
                name: info.name.clone(),
                plan_name: info.plan.name.clone(),
                version: info.plan.version,
                release: info.plan.release,
                epoch: info.plan.epoch,
                path: path.to_path_buf(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use wright_part::archive::{PartHooks, PartSpec, PlanMetadata, Provenance, write_part};

    /// Seal a `prism-<version>` archive for `plan` into the plan's
    /// subdirectory of `parts_dir`, mirroring the current layout.
    fn seal_prism(parts_dir: &std::path::Path, plan: &str, version: &str) -> PathBuf {
        let staging = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(staging.path().join("usr/lib")).unwrap();
        std::fs::write(staging.path().join("usr/lib/prism"), plan).unwrap();
        let output_dir = parts_dir.join(plan);
        std::fs::create_dir_all(&output_dir).unwrap();
        let spec = PartSpec {
            archive_name: format!("prism-{version}-1-x86_64.wright.tar.zst"),
            name: "prism".to_string(),
            runtime_deps: Vec::new(),
            replaces: Vec::new(),
            conflicts: Vec::new(),
            backup_files: Vec::new(),
            plan: PlanMetadata {
                name: plan.to_string(),
                version: version.to_string(),
                release: 1,
                epoch: 0,
                arch: "x86_64".to_string(),
            },
            provenance: Provenance {
                plan_checksum: None,
                source_checksums: Vec::new(),
                wright_version: env!("CARGO_PKG_VERSION").to_string(),
                isolation: "strict".to_string(),
            },
            plan_source: None,
            hooks: PartHooks::default(),
        };
        write_part(staging.path(), &spec, &output_dir).unwrap()
    }

    #[tokio::test]
    async fn prune_keeps_newest_per_plan_not_per_name() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        // Two plans ship a same-named output: plan "a" has an old and a
        // new build, plan "b" has only an old build. Pruning must drop
        // a's old build without touching b's only archive.
        let a_old = seal_prism(&parts_dir, "a", "1.0.0");
        let a_new = seal_prism(&parts_dir, "a", "2.0.0");
        let b_only = seal_prism(&parts_dir, "b", "1.0.0");

        let mut config = GlobalConfig::default();
        config.general.parts_dir = parts_dir;
        execute_prune(true, &config).await.unwrap();

        assert!(!a_old.exists(), "obsolete archive of plan a pruned");
        assert!(a_new.exists(), "newest archive of plan a kept");
        assert!(
            b_only.exists(),
            "another plan's same-named archive is never an old version"
        );
    }
}
