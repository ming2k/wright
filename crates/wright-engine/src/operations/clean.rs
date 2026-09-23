use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_part::archive;
use wright_part::store::ResolvedPartVersioned;

/// One `wright clean` invocation. A struct rather than a positional bool list
/// so each location's predicate stays legible at the call site and adding a
/// location never silently reorders arguments.
pub struct CleanRequest<'a> {
    /// Plans to scope to; empty means every plan.
    pub plans: &'a [String],
    /// Delete built part archives of the scoped plans.
    pub archives: bool,
    /// Delete only superseded archive versions (mutually exclusive with
    /// `archives`).
    pub stale: bool,
    /// Delete command log files.
    pub logs: bool,
    /// Delete CAS store entries not linked to any live archive.
    pub store: bool,
    /// Delete cached source files.
    pub sources: bool,
    /// With `sources`: only entries older than this many days.
    pub older_than_days: Option<u64>,
    /// Rotate the audit ledger (build records and snapshots).
    pub ledger: bool,
    /// With `ledger`: build records to keep per plan.
    pub keep_builds: usize,
    /// With `ledger`: snapshots to keep per plan (and per detached group).
    pub keep_snapshots: usize,
    /// Preview without deleting.
    pub dry_run: bool,
}

impl Default for CleanRequest<'_> {
    fn default() -> Self {
        Self {
            plans: &[],
            archives: false,
            stale: false,
            logs: false,
            store: false,
            sources: false,
            older_than_days: None,
            ledger: false,
            keep_builds: 10,
            keep_snapshots: 10,
            dry_run: false,
        }
    }
}

pub async fn execute_clean(request: CleanRequest<'_>, config: &GlobalConfig) -> Result<()> {
    let plans = request.plans;
    let archives = request.archives;
    let stale = request.stale;
    let logs = request.logs;
    let dry_run = request.dry_run;

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
    if !stale && !request.store && !request.sources && !request.ledger {
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

    let store_files = if request.store {
        reclaimable_store_entries(&config.general.store_dir, &config.general.parts_dir)
    } else {
        Vec::new()
    };

    let source_files = if request.sources {
        reclaimable_sources(&config.general.source_dir, request.older_than_days)?
    } else {
        Vec::new()
    };

    let ledger_plan = if request.ledger {
        plan_ledger(
            &config.general.ledger_dir,
            request.keep_builds,
            request.keep_snapshots,
        )?
    } else {
        LedgerPlan {
            rotate_builds: Vec::new(),
            remove_snapshots: Vec::new(),
        }
    };
    let ledger_paths = ledger_plan.paths();

    if dry_run {
        let total = freed_bytes(
            workspaces
                .iter()
                .chain(&archive_files)
                .chain(&log_files)
                .chain(&source_files)
                .chain(ledger_paths.iter()),
        ) + store_freed_bytes(&store_files);
        crate::cli_action!(
            "Dry-run",
            "{} workspace(s), {} archive(s), {} log entry(s), {} store entry(s), {} source(s), {} ledger file(s) would be removed for {target} ({})",
            workspaces.len(),
            archive_files.len(),
            log_files.len(),
            store_files.len(),
            source_files.len(),
            ledger_paths.len(),
            crate::util::display::format_bytes(total),
        );
        for path in workspaces
            .iter()
            .chain(&archive_files)
            .chain(&log_files)
            .chain(&source_files)
            .chain(ledger_paths.iter())
            .chain(store_files.iter().map(|entry| &entry.path))
        {
            crate::cli_output!("  {}", path.display());
        }
        return Ok(());
    }

    let mut freed = 0u64;

    if !stale && !workspaces.is_empty() {
        crate::cli_action!("Cleaning", "build workspaces for {target}");
        freed += freed_bytes(workspaces.iter());
        for path in &workspaces {
            // Mount-aware deletion, identical to `Foundry::clean`.
            crate::foundry::layers::force_clean_dir(path).await?;
        }
    }

    if !archive_files.is_empty() {
        crate::cli_action!("Cleaning", "part archives for {target}");
        freed += freed_bytes(archive_files.iter());
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
        freed += freed_bytes(log_files.iter());
        for path in &log_files {
            remove_entry(path)?;
        }
    }

    if !store_files.is_empty() {
        crate::cli_action!(
            "Cleaning",
            "CAS store entries in {}",
            config.general.store_dir.display()
        );
        freed += store_freed_bytes(&store_files);
        for entry in &store_files {
            if let Err(e) = std::fs::remove_file(&entry.path) {
                crate::cli_warn!("failed to remove {}: {}", entry.path.display(), e);
            }
        }
    }

    if !source_files.is_empty() {
        crate::cli_action!(
            "Cleaning",
            "cached sources in {}",
            config.general.source_dir.display()
        );
        freed += freed_bytes(source_files.iter());
        for path in &source_files {
            remove_entry(path)?;
        }
    }

    if !ledger_plan.is_empty() {
        crate::cli_action!(
            "Cleaning",
            "audit ledger in {}",
            config.general.ledger_dir.display()
        );
        freed += freed_bytes(ledger_paths.iter());
        for (path, keep) in &ledger_plan.rotate_builds {
            if let Err(e) = rotate_build_records(path, *keep) {
                crate::cli_warn!("failed to rotate {}: {}", path.display(), e);
            }
        }
        for path in &ledger_plan.remove_snapshots {
            if let Err(e) = std::fs::remove_file(path) {
                crate::cli_warn!("failed to remove {}: {}", path.display(), e);
            }
        }
    }

    crate::cli_output!(
        "Removed {} workspace(s), {} archive(s), {} log entry(s), {} store entry(s), {} source(s), and {} ledger file(s) ({} reclaimed).",
        workspaces.len(),
        archive_files.len(),
        log_files.len(),
        store_files.len(),
        source_files.len(),
        ledger_paths.len(),
        crate::util::display::format_bytes(freed),
    );
    Ok(())
}

/// Sum apparent bytes of a set of existing files (hard links counted once per
/// inode, so a set that shares inodes does not over-report).
fn freed_bytes<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> u64 {
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut bytes = 0u64;
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if meta.is_dir() {
            let (dir_bytes, _) = crate::ledger::dir_stats(path);
            bytes = bytes.saturating_add(dir_bytes);
            continue;
        }
        if seen.insert((meta.dev(), meta.ino())) {
            bytes = bytes.saturating_add(meta.len());
        }
    }
    bytes
}

/// A store entry selected for removal, with the bytes its removal frees.
struct StoreEntry {
    path: PathBuf,
    /// Bytes freed once per inode; the first entry for an inode carries the
    /// size, its hard-linked twins carry zero.
    freed: u64,
}

fn store_freed_bytes(entries: &[StoreEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| entry.freed)
        .fold(0u64, u64::saturating_add)
}

/// Store entries whose inode is shared with no live `parts_dir` file.
///
/// The CAS store is a pure rebuild cache; a `parts_dir` archive is the copy
/// that matters. An entry hard-linked to a live archive frees nothing when
/// removed, so it is left alone. An entry whose inode survives only in the
/// store is an orphaned cache copy, and removing it frees real space — counted
/// once per inode, since two orphaned entries may hard-link to each other.
fn reclaimable_store_entries(store_dir: &Path, parts_dir: &Path) -> Vec<StoreEntry> {
    if !store_dir.exists() {
        return Vec::new();
    }

    let mut live: HashSet<(u64, u64)> = HashSet::new();
    if parts_dir.exists() {
        for entry in walkdir::WalkDir::new(parts_dir).into_iter().flatten() {
            if entry.file_type().is_file()
                && let Ok(meta) = entry.metadata()
            {
                live.insert((meta.dev(), meta.ino()));
            }
        }
    }

    let mut counted: HashSet<(u64, u64)> = HashSet::new();
    let mut selected = Vec::new();
    for entry in walkdir::WalkDir::new(store_dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let key = (meta.dev(), meta.ino());
        if live.contains(&key) {
            continue;
        }
        let freed = if counted.insert(key) { meta.len() } else { 0 };
        selected.push(StoreEntry {
            path: entry.path().to_path_buf(),
            freed,
        });
    }
    selected.sort_by(|a, b| a.path.cmp(&b.path));
    selected
}

/// Cached source files selected for removal. `older_than_days` narrows by
/// modification time; `None` selects every cached source (they are a pure
/// cache and re-fetchable).
fn reclaimable_sources(source_dir: &Path, older_than_days: Option<u64>) -> Result<Vec<PathBuf>> {
    if !source_dir.exists() {
        return Ok(Vec::new());
    }
    validate_cleanup_root(source_dir)?;

    let cutoff = older_than_days.map(|days| {
        std::time::SystemTime::now() - std::time::Duration::from_secs(days.saturating_mul(86_400))
    });

    let mut selected = Vec::new();
    for entry in walkdir::WalkDir::new(source_dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(cutoff) = cutoff {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            if modified >= cutoff {
                continue;
            }
        }
        selected.push(entry.path().to_path_buf());
    }
    selected.sort();
    Ok(selected)
}

/// Audit-ledger reclamation, split into selection and application so a
/// dry run never mutates.
struct LedgerPlan {
    /// Build-record files to rewrite, with the number of trailing records to
    /// keep.
    rotate_builds: Vec<(PathBuf, usize)>,
    /// Snapshot files to delete.
    remove_snapshots: Vec<PathBuf>,
}

impl LedgerPlan {
    fn is_empty(&self) -> bool {
        self.rotate_builds.is_empty() && self.remove_snapshots.is_empty()
    }

    /// Every path the plan touches, for listing and byte accounting.
    fn paths(&self) -> Vec<PathBuf> {
        self.rotate_builds
            .iter()
            .map(|(path, _)| path.clone())
            .chain(self.remove_snapshots.iter().cloned())
            .collect()
    }
}

/// Select ledger files to rotate: build records beyond `keep_builds` per plan,
/// and snapshots beyond `keep_snapshots` per plan (and per detached group).
fn plan_ledger(ledger_dir: &Path, keep_builds: usize, keep_snapshots: usize) -> Result<LedgerPlan> {
    let mut plan = LedgerPlan {
        rotate_builds: Vec::new(),
        remove_snapshots: Vec::new(),
    };
    if !ledger_dir.exists() {
        return Ok(plan);
    }
    validate_cleanup_root(ledger_dir)?;

    let mut build_files = Vec::new();
    let mut snapshot_dirs = Vec::new();
    for entry in walkdir::WalkDir::new(ledger_dir).into_iter().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_file() && name == "builds.jsonl" {
            build_files.push(entry.path().to_path_buf());
        } else if entry.file_type().is_dir() && (name == "snapshots" || name == "_detached") {
            snapshot_dirs.push(entry.path().to_path_buf());
        }
    }

    for path in build_files {
        if build_record_lines(&path)? > keep_builds {
            plan.rotate_builds.push((path, keep_builds));
        }
    }
    for dir in snapshot_dirs {
        plan.remove_snapshots
            .extend(excess_snapshots(&dir, keep_snapshots)?);
    }

    plan.rotate_builds.sort();
    plan.remove_snapshots.sort();
    Ok(plan)
}

fn build_record_lines(path: &Path) -> Result<usize> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content.lines().count()),
        Err(_) => Ok(0),
    }
}

/// Rewrite a build-record file to its last `keep` lines.
fn rotate_build_records(path: &Path, keep: usize) -> Result<()> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| WrightError::context(format!("failed to read {}", path.display()), e))?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= keep {
        return Ok(());
    }
    let mut out = lines[lines.len() - keep..].join("\n");
    out.push('\n');
    std::fs::write(path, out)
        .map_err(|e| WrightError::context(format!("failed to rotate {}", path.display()), e))?;
    Ok(())
}

/// Snapshot files to delete so only the `keep` newest remain. Filenames are
/// `<yyyymmddThhmmssZ>-<checksum>.toml`, so lexical order is chronological.
fn excess_snapshots(dir: &Path, keep: usize) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(WrightError::context(
                format!("failed to read {}", dir.display()),
                e,
            ));
        }
    };
    let mut snapshots: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    snapshots.sort();
    if snapshots.len() <= keep {
        return Ok(Vec::new());
    }
    let excess = snapshots.len() - keep;
    Ok(snapshots.into_iter().take(excess).collect())
}

/// Archive files superseded by a newer version of the same (plan, output)
/// pair. When `plan_names` is non-empty, only those plans are considered.
/// Identity comes from `.PARTINFO`, so unrelated plans shipping same-named
/// outputs never obsolete each other's archives.
pub(crate) fn stale_archives(parts_dir: &Path, plan_names: &[&str]) -> Result<Vec<PathBuf>> {
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

    fn request<'a>(plans: &'a [String], stale: bool, dry_run: bool) -> CleanRequest<'a> {
        CleanRequest {
            plans,
            stale,
            dry_run,
            ..Default::default()
        }
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

        execute_clean(request(&[], true, false), &test_config(parts_dir))
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

        execute_clean(request(&[], true, true), &test_config(parts_dir))
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
        execute_clean(request(&[target], true, false), &config)
            .await
            .unwrap();

        assert!(!a_old.exists(), "stale archive of the named plan removed");
        assert!(a_new.exists(), "newest archive of the named plan kept");
        assert!(b_old.exists(), "unnamed plan's stale archive survives");
        assert!(b_new.exists(), "unnamed plan's newest archive survives");
    }

    #[test]
    fn store_clean_removes_only_orphaned_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let parts = tmp.path().join("parts");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::create_dir_all(&store).unwrap();

        // A live archive with its hard-linked CAS entry: removing the entry
        // frees nothing, so it must be left alone.
        let archive = parts.join("demo.wright.tar.zst");
        std::fs::write(&archive, vec![0u8; 100]).unwrap();
        let linked = store.join("aaaaaaaaaaaaaaaa-demo.part");
        std::fs::hard_link(&archive, &linked).unwrap();

        // A standalone orphan cache copy: removed, freeing its bytes.
        let orphan = store.join("bbbbbbbbbbbbbbbb-demo.part");
        std::fs::write(&orphan, vec![0u8; 40]).unwrap();

        let entries = reclaimable_store_entries(&store, &parts);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, orphan);
        assert_eq!(store_freed_bytes(&entries), 40);
        assert!(linked.exists(), "linked entry is not selected");
    }

    #[test]
    fn ledger_rotation_keeps_recent_builds_and_snapshots() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path().join("ledger");
        let plan_dir = ledger.join("demo");
        std::fs::create_dir_all(&plan_dir).unwrap();

        let builds = plan_dir.join("builds.jsonl");
        let mut content = String::new();
        for i in 0..5 {
            content.push_str(&format!("{{\"n\":{i}}}\n"));
        }
        std::fs::write(&builds, content).unwrap();

        let snap_dir = plan_dir.join("snapshots");
        std::fs::create_dir_all(&snap_dir).unwrap();
        for stamp in ["20250101T000000Z", "20250201T000000Z", "20250301T000000Z"] {
            std::fs::write(snap_dir.join(format!("{stamp}-abc.toml")), "x").unwrap();
        }

        // Selection must not mutate: the plan is a pure preview.
        let plan = plan_ledger(&ledger, 2, 2).unwrap();
        assert_eq!(plan.rotate_builds.len(), 1);
        assert_eq!(plan.remove_snapshots.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&builds).unwrap().lines().count(),
            5,
            "planning must not rotate"
        );
        assert_eq!(std::fs::read_dir(&snap_dir).unwrap().count(), 3);

        // Applying the plan rotates in place and removes the excess snapshot.
        rotate_build_records(&plan.rotate_builds[0].0, plan.rotate_builds[0].1).unwrap();
        for path in &plan.remove_snapshots {
            std::fs::remove_file(path).unwrap();
        }

        let content = std::fs::read_to_string(&builds).unwrap();
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains("\"n\":3"));
        assert!(content.contains("\"n\":4"));

        let remaining: Vec<String> = std::fs::read_dir(&snap_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.iter().any(|n| n.starts_with("20250101")));
    }
}
