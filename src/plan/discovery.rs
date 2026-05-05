use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::error::{Result, WrightError};
use crate::plan::manifest::PlanManifest;

pub fn collect_plan_files(plans_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for root in plans_dirs {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root) {
            let entry = entry.map_err(|e| WrightError::IoError(std::io::Error::other(e)))?;
            if entry.file_name() == "plan.toml" {
                let path = entry.path().to_path_buf();
                if seen.insert(path.clone()) {
                    paths.push(path);
                }
            }
        }
    }

    paths.sort();
    Ok(paths)
}

pub fn get_all_plans(plans_dirs: &[PathBuf]) -> Result<HashMap<String, PathBuf>> {
    let mut map = HashMap::new();
    for path in collect_plan_files(plans_dirs)? {
        if let Ok(manifest) = PlanManifest::from_file(&path) {
            map.insert(manifest.metadata.name, path);
        }
    }
    Ok(map)
}
