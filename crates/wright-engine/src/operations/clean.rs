use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_part::archive;
use wright_part::store::ResolvedPartVersioned;

pub async fn execute_clean(
    plans: &[String],
    archives: bool,
    stale: bool,
    logs: bool,
    dry_run: bool,
    config: &GlobalConfig,
) -> Result<()> {
    // The index is only needed to resolve named plans; bulk cleaning skips it.
    let manifests = if plans.is_empty() {
        Vec::new()
    } else {
        let index = super::targets::plan_index(config)?;
        plans
            .iter()
            .map(|target| super::targets::load_manifest(target, &index))
            .collect::<Result<Vec<_>>>()?
    };
    let plan_names: Vec<&str> = manifests
        .iter()
        .map(|manifest| manifest.metadata.name.as_str())
        .collect();
    let target = if plan_names.is_empty() {
        "all plans".to_string()
    } else {
        plan_names.join(", ")
    };

    // Collect before deleting so --dry-run reports the exact same set.
    let mut workspaces = Vec::<PathBuf>::new();
    if !stale {
        if manifests.is_empty() {
            workspaces = matching_entries(&config.build.forge_dir, |path, _name| path.is_dir())?;
        } else {
            let foundry = crate::foundry::Foundry::new(config.clone());
            for manifest in &manifests {
                let build_root = foundry.build_root(manifest)?;
                if build_root.exists() {
                    workspaces.push(build_root);
                }
            }
        }
    }

    let archive_files = if stale {
        stale_archives(&config.general.parts_dir, &plan_names)?
    } else if archives {
        matching_part_archives(&config.general.parts_dir, &plan_names)?
    } else {
        Vec::new()
    };

    let log_files = if logs {
        matching_entries(&config.general.logs_dir, |_path, _name| true)?
    } else {
        Vec::new()
    };

    if dry_run {
        crate::cli_action!(
            "Dry-run",
            "{} workspace(s), {} archive(s), {} log entry(s) would be removed for {target}",
            workspaces.len(),
            archive_files.len(),
            log_files.len()
        );
        for path in workspaces.iter().chain(&archive_files).chain(&log_files) {
            crate::cli_output!("  {}", path.display());
        }
        return Ok(());
    }

    if !stale {
        crate::cli_action!("Cleaning", "build workspaces for {target}");
        for path in &workspaces {
            // Mount-aware deletion, identical to `Foundry::clean`.
            crate::foundry::layers::force_clean_dir(path).await?;
        }
    }

    if !archive_files.is_empty() {
        crate::cli_action!("Cleaning", "part archives for {target}");
        for path in &archive_files {
            remove_entry(path)?;
        }
        remove_emptied_plan_dirs(&config.general.parts_dir, &plan_names)?;
    }

    if !log_files.is_empty() {
        crate::cli_action!(
            "Cleaning",
            "command logs in {}",
            config.general.logs_dir.display()
        );
        for path in &log_files {
            remove_entry(path)?;
        }
    }

    crate::cli_output!(
        "Removed {} workspace(s), {} archive(s), and {} log entry(s).",
        workspaces.len(),
        archive_files.len(),
        log_files.len()
    );
    Ok(())
}

/// Archive files superseded by a newer version of the same (plan, output)
/// pair. When `plan_names` is non-empty, only those plans are considered.
/// Identity comes from `.PARTINFO`, so unrelated plans shipping same-named
/// outputs never obsolete each other's archives.
fn stale_archives(parts_dir: &Path, plan_names: &[&str]) -> Result<Vec<PathBuf>> {
    if !parts_dir.exists() {
        return Ok(Vec::new());
    }
    validate_cleanup_root(parts_dir)?;

    let mut parts_by_name: HashMap<(String, String), Vec<ResolvedPartVersioned>> = HashMap::new();
    // Recurse: current archives live in per-plan subdirectories, older
    // ones flat at the top level.
    for entry in walkdir::WalkDir::new(parts_dir).into_iter().flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".wright.tar.zst"))
        {
            continue;
        }
        let Ok(info) = archive::read_partinfo(path) else {
            continue;
        };
        if !plan_names.is_empty() && !plan_names.contains(&info.plan.name.as_str()) {
            continue;
        }
        let versioned = ResolvedPartVersioned {
            name: info.name.clone(),
            plan_name: info.plan.name.clone(),
            version: info.plan.version,
            release: info.plan.release,
            epoch: info.plan.epoch,
            path: path.to_path_buf(),
            dependencies: info.runtime_deps,
        };
        parts_by_name
            .entry((info.plan.name, info.name))
            .or_default()
            .push(versioned);
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
    Ok(candidates)
}

/// Collect part archives belonging to `plan_names` (every archive when
/// empty). Current archives live in per-plan subdirectories and older ones
/// flat at the top level, so the walk recurses. Matching is by originating
/// plan only — an output that merely shares the plan's name belongs to
/// another plan and must survive.
fn matching_part_archives(parts_dir: &Path, plan_names: &[&str]) -> Result<Vec<PathBuf>> {
    if !parts_dir.exists() {
        return Ok(Vec::new());
    }
    validate_cleanup_root(parts_dir)?;

    let mut matches = Vec::<PathBuf>::new();
    for entry in walkdir::WalkDir::new(parts_dir).into_iter().flatten() {
        let path = entry.path();
        let is_archive = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".wright.tar.zst"));
        if !is_archive {
            continue;
        }
        let belongs = plan_names.is_empty()
            || archive::read_partinfo(path)
                .is_ok_and(|info| plan_names.iter().any(|plan| info.plan.name == *plan));
        if belongs {
            matches.push(path.to_path_buf());
        }
    }
    matches.sort();
    Ok(matches)
}

/// Drop plan subdirectories an archive deletion just emptied (`remove_dir`
/// refuses non-empty directories, so foreign content is safe).
fn remove_emptied_plan_dirs(parts_dir: &Path, plan_names: &[&str]) -> Result<()> {
    for entry in std::fs::read_dir(parts_dir).map_err(|error| io_error(parts_dir, error))? {
        let entry = entry.map_err(|error| io_error(parts_dir, error))?;
        let path = entry.path();
        let name = entry.file_name();
        let targeted =
            plan_names.is_empty() || name.to_str().is_some_and(|name| plan_names.contains(&name));
        if targeted && metadata_is_plain_dir(&entry) {
            let _ = std::fs::remove_dir(&path);
        }
    }
    Ok(())
}

fn metadata_is_plain_dir(entry: &std::fs::DirEntry) -> bool {
    entry
        .file_type()
        .map(|file_type| file_type.is_dir() && !file_type.is_symlink())
        .unwrap_or(false)
}

fn matching_entries(
    directory: &Path,
    mut predicate: impl FnMut(&Path, &str) -> bool,
) -> Result<Vec<PathBuf>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    validate_cleanup_root(directory)?;

    let mut matches = Vec::<PathBuf>::new();
    for entry in std::fs::read_dir(directory).map_err(|error| io_error(directory, error))? {
        let entry = entry.map_err(|error| io_error(directory, error))?;
        let path = entry.path();
        let name = entry.file_name();
        if let Some(name) = name.to_str()
            && predicate(&path, name)
        {
            matches.push(path);
        }
    }
    matches.sort();
    Ok(matches)
}

fn validate_cleanup_root(directory: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(directory).map_err(|error| io_error(directory, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WrightError::ValidationError(format!(
            "cleanup root must be a directory, not a symlink or file: {}",
            directory.display()
        )));
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| io_error(path, error))?;
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|error| io_error(path, error))
}

fn io_error(path: &Path, error: std::io::Error) -> WrightError {
    WrightError::IoError(std::io::Error::new(
        error.kind(),
        format!("{}: {error}", path.display()),
    ))
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
            build_info: None,
            hooks: PartHooks::default(),
        };
        write_part(staging.path(), &spec, &output_dir).unwrap()
    }

    fn test_config(parts_dir: PathBuf) -> GlobalConfig {
        let mut config = GlobalConfig::default();
        config.general.parts_dir = parts_dir;
        config
    }

    #[tokio::test]
    async fn stale_keeps_newest_per_plan_not_per_name() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        // Two plans ship a same-named output: plan "a" has an old and a
        // new build, plan "b" has only an old build. Stale cleaning must
        // drop a's old build without touching b's only archive.
        let a_old = seal_prism(&parts_dir, "a", "1.0.0");
        let a_new = seal_prism(&parts_dir, "a", "2.0.0");
        let b_only = seal_prism(&parts_dir, "b", "1.0.0");

        execute_clean(&[], false, true, false, false, &test_config(parts_dir))
            .await
            .unwrap();

        assert!(!a_old.exists(), "obsolete archive of plan a removed");
        assert!(a_new.exists(), "newest archive of plan a kept");
        assert!(
            b_only.exists(),
            "another plan's same-named archive is never stale"
        );
    }

    #[tokio::test]
    async fn dry_run_removes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        let a_old = seal_prism(&parts_dir, "a", "1.0.0");
        let a_new = seal_prism(&parts_dir, "a", "2.0.0");

        execute_clean(&[], false, true, false, true, &test_config(parts_dir))
            .await
            .unwrap();

        assert!(a_old.exists(), "dry run keeps the stale archive");
        assert!(a_new.exists(), "dry run keeps the newest archive");
    }

    #[tokio::test]
    async fn stale_filters_to_named_plans() {
        let tmp = tempfile::tempdir().unwrap();
        let parts_dir = tmp.path().join("parts");
        std::fs::create_dir(&parts_dir).unwrap();

        // Plan directories double as clean targets (`load_manifest` accepts
        // a directory containing plan.toml).
        let plans_dir = tmp.path().join("plans");
        let plan_a = plans_dir.join("a");
        std::fs::create_dir_all(&plan_a).unwrap();
        std::fs::write(
            plan_a.join("plan.toml"),
            "name = \"a\"\nversion = \"2.0.0\"\nrelease = 1\ndescription = \"test plan a\"\nlicense = \"MIT\"\narch = \"x86_64\"\n",
        )
        .unwrap();

        let a_old = seal_prism(&parts_dir, "a", "1.0.0");
        let a_new = seal_prism(&parts_dir, "a", "2.0.0");
        let b_old = seal_prism(&parts_dir, "b", "1.0.0");
        let b_new = seal_prism(&parts_dir, "b", "2.0.0");

        let mut config = test_config(parts_dir);
        config.general.plans_dir = plans_dir;
        let target = plan_a.to_string_lossy().into_owned();
        execute_clean(&[target], false, true, false, false, &config)
            .await
            .unwrap();

        assert!(!a_old.exists(), "stale archive of the named plan removed");
        assert!(a_new.exists(), "newest archive of the named plan kept");
        assert!(b_old.exists(), "unnamed plan's stale archive survives");
        assert!(b_new.exists(), "unnamed plan's newest archive survives");
    }
}
