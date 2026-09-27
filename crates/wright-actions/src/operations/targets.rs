use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_plan::{PlanIndex, PlanManifest};

pub(super) fn plan_index(config: &GlobalConfig) -> Result<PlanIndex> {
    Ok(PlanIndex::discover(&crate::resolve::plan_search_dirs(
        config,
    ))?)
}

pub(super) fn load_manifest(target: &str, index: &PlanIndex) -> Result<PlanManifest> {
    let target_path = Path::new(target);
    let manifest_path = if target_path.is_dir() {
        target_path.join("plan.toml")
    } else {
        PathBuf::from(target_path)
    };

    if manifest_path.is_file() {
        return PlanManifest::from_file(&manifest_path).map_err(Into::into);
    }
    if let Some(path) = index.path_for(target) {
        return PlanManifest::from_file(path).map_err(Into::into);
    }

    Err(WrightError::PartNotFound(format!(
        "plan manifest not found for '{target}'"
    )))
}
