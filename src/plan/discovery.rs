use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};
use crate::plan::manifest::PlanManifest;

/// Walk plan directories and collect every `plan.toml` path found.
///
/// This is a pure filesystem operation: it does **not** parse files.
pub fn discover_plan_paths(plan_dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for root in plan_dirs {
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

/// Lightweight index of discovered plans.
///
/// Building the index parses only the `name` field of each `plan.toml` to
/// establish the `name → path` mapping.  Full `PlanManifest` parsing is
/// deferred until first access and then cached, eliminating the duplicate
/// parsing that the old `get_all_plans` + `manifest_cache` pattern suffered.
#[derive(Clone)]
pub struct PlanIndex {
    entries: HashMap<String, PathBuf>,
    cache: RefCell<HashMap<String, PlanManifest>>,
}

impl PlanIndex {
    /// Discover plans under the given directories and build the index.
    pub fn discover(plan_dirs: &[PathBuf]) -> Result<Self> {
        let mut entries = HashMap::new();
        for path in discover_plan_paths(plan_dirs)? {
            match Self::extract_name(&path) {
                Ok(name) => {
                    entries.insert(name, path);
                }
                Err(e) => {
                    tracing::warn!("Skipping invalid plan file {}: {}", path.display(), e);
                }
            }
        }
        Ok(Self {
            entries,
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// Build an index from a pre-populated map.  Useful for tests.
    pub fn from_entries(entries: HashMap<String, PathBuf>) -> Self {
        Self {
            entries,
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// Look up the filesystem path for a plan by its declared name.
    pub fn path_for(&self, name: &str) -> Option<&PathBuf> {
        self.entries.get(name)
    }

    /// Parse (or return cached) the full manifest for a plan by name.
    pub fn manifest_for(&self, name: &str) -> Result<Option<PlanManifest>> {
        if let Some(path) = self.entries.get(name) {
            if let Some(cached) = self.cache.borrow().get(name) {
                return Ok(Some(cached.clone()));
            }
            let manifest = PlanManifest::from_file(path)?;
            self.cache
                .borrow_mut()
                .insert(name.to_string(), manifest.clone());
            Ok(Some(manifest))
        } else {
            Ok(None)
        }
    }

    /// Eagerly parse every plan in the index and return `(name, manifest)` pairs.
    ///
    /// Callers that genuinely need a full view (e.g. `wright lint`) should use
    /// this instead of forcing every other workflow to pay the parsing cost.
    pub fn load_all(&self) -> Result<Vec<(String, PlanManifest)>> {
        let mut results = Vec::with_capacity(self.entries.len());
        for name in self.entries.keys() {
            match self.manifest_for(name) {
                Ok(Some(manifest)) => results.push((name.clone(), manifest)),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("Skipping plan '{}' during bulk load: {}", name, e);
                }
            }
        }
        Ok(results)
    }

    /// Iterate over all known plan names.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    /// Iterate over all discovered plan paths.
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.entries.values()
    }

    /// Number of indexed plans.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Lightweight extraction of the `name` field without deserialising the
    /// full manifest.  This is the only parsing cost paid at index-build time.
    fn extract_name(path: &Path) -> Result<String> {
        let content = std::fs::read_to_string(path).map_err(|e| WrightError::IoError(e))?;
        #[derive(serde::Deserialize)]
        struct NameOnly {
            name: String,
        }
        let partial: NameOnly = toml::from_str(&content).map_err(|e| {
            WrightError::ValidationError(format!("invalid plan.toml '{}': {}", path.display(), e))
        })?;
        Ok(partial.name)
    }
}
