use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};

pub async fn execute_clean(
    plans: &[String],
    parts: bool,
    logs: bool,
    config: &GlobalConfig,
) -> Result<()> {
    let index = super::targets::plan_index(config)?;
    let manifests = plans
        .iter()
        .map(|target| super::targets::load_manifest(target, &index))
        .collect::<Result<Vec<_>>>()?;
    let plan_names: Vec<&str> = manifests
        .iter()
        .map(|manifest| manifest.metadata.name.as_str())
        .collect();
    let target = if plan_names.is_empty() {
        "all plans".to_string()
    } else {
        plan_names.join(", ")
    };

    crate::cli_action!("Cleaning", "build workspaces for {target}");
    let workspaces = if manifests.is_empty() {
        remove_matching_entries(&config.build.forge_dir, |path, _name| path.is_dir())?
    } else {
        let foundry = crate::foundry::Foundry::new(config.clone());
        let mut removed = 0;
        for manifest in &manifests {
            let build_root = foundry.build_root(manifest)?;
            if build_root.exists() {
                foundry.clean(manifest).await?;
                removed += 1;
            }
        }
        removed
    };

    let archives = if parts {
        crate::cli_action!(
            "Cleaning",
            "part archives in {}",
            config.general.parts_dir.display()
        );
        remove_matching_entries(&config.general.parts_dir, |path, name| {
            name.ends_with(".wright.tar.zst")
                && (plan_names.is_empty()
                    || wright_part::archive::read_partinfo(path).is_ok_and(|info| {
                        plan_names
                            .iter()
                            .any(|plan| info.plan.name == **plan || info.name == **plan)
                    }))
        })?
    } else {
        0
    };

    let log_files = if logs {
        crate::cli_action!(
            "Cleaning",
            "command logs in {}",
            config.general.logs_dir.display()
        );
        remove_matching_entries(&config.general.logs_dir, |_path, _name| true)?
    } else {
        0
    };

    crate::cli_output!(
        "Removed {workspaces} workspace(s), {archives} archive(s), and {log_files} log entry(s)."
    );
    Ok(())
}

fn remove_matching_entries(
    directory: &Path,
    mut predicate: impl FnMut(&Path, &str) -> bool,
) -> Result<usize> {
    if !directory.exists() {
        return Ok(0);
    }
    let metadata =
        std::fs::symlink_metadata(directory).map_err(|error| io_error(directory, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(WrightError::ValidationError(format!(
            "cleanup root must be a directory, not a symlink or file: {}",
            directory.display()
        )));
    }

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

    for path in &matches {
        remove_entry(path)?;
    }
    Ok(matches.len())
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
