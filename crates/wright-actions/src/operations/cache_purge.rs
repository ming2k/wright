//! Transactional cold-cache purge (ADR-0057).
//!
//! A cold build discards the pure caches — the CAS store and the source cache —
//! without ever touching `parts_dir`, the registry, the ledger, or logs. The
//! purge is staged per root under a hidden directory, journaled, and then
//! committed, so a crash or an I/O failure cannot leave a half-purged cache.
//!
//! Roots are staged **store first, source second**. Any intermediate state is
//! then safe: an empty CAS with a full or partial source cache can only cause a
//! rebuild or a re-fetch, never the reuse of a stale artifact against new
//! source bytes.
//!
//! `http` sources with a concrete `sha256` are inputs, not caches. When the
//! cached bytes still match the declaration they are kept (and reused by
//! charge); everything unverifiable — git named-ref snapshots, `http` `SKIP`
//! entries, local cache copies — is discarded.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use crate::foundry::charge::pinned_cache_entries;
use crate::operations::clean::{io_error, reclaimed_bytes, validate_cleanup_root};
use crate::util::checksum;
use wright_plan::manifest::PlanManifest;
use wright_resolve::BuildExecutionPlan;

const JOURNAL_VERSION: u32 = 1;
const STAGE_PREFIX: &str = ".wright-purge-";

#[derive(Debug)]
pub(crate) struct PurgePlan {
    txid: String,
    journal_dir: PathBuf,
    /// Ordered store-first, then source. Roots with no selected entries are
    /// omitted.
    roots: Vec<RootPlan>,
    paths: usize,
    reclaimed: u64,
    kept: usize,
}

#[derive(Debug)]
struct RootPlan {
    root: PathBuf,
    stage: PathBuf,
    /// Paths relative to `root`; a directory is expanded during staging.
    entries: Vec<PathBuf>,
}

impl PurgePlan {
    pub(crate) fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub(crate) fn reclaimed(&self) -> u64 {
        self.reclaimed
    }
}

/// Plan a cold purge, keeping verified pinned `http` inputs referenced by the
/// selected build set.
pub(crate) fn plan(
    config: &GlobalConfig,
    db_path: &Path,
    keep: &[(PathBuf, String)],
    parts_dir: &Path,
) -> Result<PurgePlan> {
    validate_layout(config, db_path)?;

    let mut keep_map: HashMap<&Path, Vec<&str>> = HashMap::new();
    for (path, hash) in keep {
        keep_map
            .entry(path.as_path())
            .or_default()
            .push(hash.as_str());
    }

    let mut selections: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    let mut kept = 0;

    // Store first: invalidation before inputs.
    if let Some(entries) = root_entries(&config.general.store_dir, None, &mut kept)? {
        selections.push((config.general.store_dir.clone(), entries));
    }
    if let Some(entries) = root_entries(&config.general.source_dir, Some(&keep_map), &mut kept)? {
        selections.push((config.general.source_dir.clone(), entries));
    }

    make_plan(db_path, parts_dir, selections, kept)
}

/// Plan a purge from explicit `(root, absolute paths)` selections. Used by
/// `wright clean`, whose predicates select individual files rather than whole
/// cache roots.
pub(crate) fn plan_entries(
    config: &GlobalConfig,
    db_path: &Path,
    parts_dir: &Path,
    mut selections: Vec<(PathBuf, Vec<PathBuf>)>,
) -> Result<PurgePlan> {
    validate_layout(config, db_path)?;

    selections.retain(|(_, entries)| !entries.is_empty());
    for (root, entries) in &mut selections {
        let root_path = root.as_path();
        validate_cleanup_root(root_path)?;
        for entry in entries.iter() {
            if !entry.starts_with(root_path) {
                return Err(WrightError::ValidationError(format!(
                    "purge entry {} is not under its cache root {}",
                    entry.display(),
                    root_path.display()
                )));
            }
        }
        entries.sort();
        entries.dedup();
    }
    // Store first, regardless of the caller's order.
    selections.sort_by_key(|(root, _)| {
        if root == &config.general.store_dir {
            0
        } else {
            1
        }
    });

    make_plan(db_path, parts_dir, selections, 0)
}

fn make_plan(
    db_path: &Path,
    parts_dir: &Path,
    selections: Vec<(PathBuf, Vec<PathBuf>)>,
    kept: usize,
) -> Result<PurgePlan> {
    let txid = Uuid::new_v4().simple().to_string();
    let journal_dir = journal_dir(db_path);
    let mut roots = Vec::new();
    let mut all = Vec::new();
    let mut paths = 0usize;

    for (root, entries) in selections {
        let mut rels = Vec::new();
        for absolute in entries {
            let rel = absolute
                .strip_prefix(&root)
                .map_err(|_| {
                    WrightError::ValidationError(format!(
                        "purge entry {} is not under {}",
                        absolute.display(),
                        root.display()
                    ))
                })?
                .to_path_buf();
            if rel.as_os_str().is_empty() {
                continue;
            }
            paths += 1;
            all.push(absolute);
            rels.push(rel);
        }
        if rels.is_empty() {
            continue;
        }
        roots.push(RootPlan {
            stage: root.join(format!("{STAGE_PREFIX}{txid}")),
            root,
            entries: rels,
        });
    }

    let reclaimed = reclaimed_bytes(&all, parts_dir);
    Ok(PurgePlan {
        txid,
        journal_dir,
        roots,
        paths,
        reclaimed,
        kept,
    })
}

fn root_entries(
    root: &Path,
    keep: Option<&HashMap<&Path, Vec<&str>>>,
    kept: &mut usize,
) -> Result<Option<Vec<PathBuf>>> {
    if !root.exists() {
        return Ok(None);
    }
    validate_cleanup_root(root)?;

    let mut selected = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| io_error(root, error))? {
        let entry = entry.map_err(|error| io_error(root, error))?;
        let path = entry.path();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGE_PREFIX)
        {
            // Recovery owns these; never treat a journaled stage as a cache
            // entry to purge again.
            continue;
        }

        let Some(keep_map) = keep else {
            selected.push(path);
            continue;
        };

        let meta = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if meta.is_file()
            && let Some(expected) = keep_map.get(path.as_path())
            && let Ok(actual) = checksum::sha256_file(&path)
            && expected.iter().any(|hash| actual == *hash)
        {
            *kept += 1;
            continue;
        }
        selected.push(path);
    }
    selected.sort();
    Ok((!selected.is_empty()).then_some(selected))
}

/// One-line summary for action output.
pub(crate) fn summary(plan: &PurgePlan) -> String {
    let kept = if plan.kept > 0 {
        format!(", {} verified input(s) kept", plan.kept)
    } else {
        String::new()
    };
    format!(
        "{} path(s) across {} cache root(s), {} reclaimed{}",
        plan.paths,
        plan.roots.len(),
        crate::util::display::format_bytes(plan.reclaimed),
        kept
    )
}

pub(crate) async fn execute(plan: &PurgePlan) -> Result<()> {
    execute_inner(plan, false).await
}

/// Execute without emitting the `Purging` action line. Used by `wright clean`,
/// which reports its own per-location summary.
pub(crate) async fn execute_silent(plan: &PurgePlan) -> Result<()> {
    execute_inner(plan, true).await
}

async fn execute_inner(plan: &PurgePlan, silent: bool) -> Result<()> {
    if plan.is_empty() {
        return Ok(());
    }
    if !silent {
        crate::cli_action!("Purging", "{}", summary(plan));
    }

    let journal_path = write_journal(plan, JournalState::Staging)?;

    // Stage roots in plan order (store first).
    for root in &plan.roots {
        if let Err(error) = stage_root(root) {
            let restored = plan.roots.iter().all(|root| rollback_stage(root).is_ok());
            if restored {
                let _ = fs::remove_file(&journal_path);
            } else {
                tracing::warn!(
                    event = "purge.rollback_failed",
                    journal = %journal_path.display(),
                    "Purge staging failed and rollback was incomplete; the journal will be retried"
                );
            }
            return Err(error);
        }
    }

    // Commit: from here on, a crash finalizes rather than restores.
    if let Err(error) = write_journal(plan, JournalState::Committed) {
        let restored = plan.roots.iter().all(|root| rollback_stage(root).is_ok());
        if restored {
            let _ = fs::remove_file(&journal_path);
        }
        return Err(error);
    }

    let mut finalized = true;
    for root in &plan.roots {
        if let Err(error) = finalize_stage(root) {
            finalized = false;
            tracing::warn!(
                event = "purge.cleanup_failed",
                stage = %root.stage.display(),
                error = %error,
                "Purge committed but its staging directory could not be removed; recovery will retry"
            );
        }
    }
    if finalized && let Err(error) = fs::remove_file(&journal_path) {
        tracing::warn!(
            event = "purge.journal_remove_failed",
            journal = %journal_path.display(),
            error = %error,
            "Purge committed but its journal could not be removed; recovery will retry"
        );
    }
    Ok(())
}

/// Restore any interrupted purge and reap committed staging residue. Idempotent.
/// Callers must hold the `wright` lock; the function touches the caches.
pub(crate) fn recover(config: &GlobalConfig, db_path: &Path) -> Result<()> {
    validate_layout(config, db_path)?;

    let dir = journal_dir(db_path);
    let mut journals = Vec::new();
    if dir.exists() {
        match fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| io_error(&dir, error))?;
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    match read_journal(&path) {
                        Ok(journal) => journals.push(journal),
                        Err(error) => tracing::warn!(
                            event = "purge.journal_unreadable",
                            journal = %path.display(),
                            error = %error,
                            "Skipping an unreadable purge journal; its staging residue will be treated as committed"
                        ),
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                tracing::warn!(
                    event = "purge.journal_dir_unreadable",
                    path = %dir.display(),
                    error = %error,
                    "Cannot inspect the purge journal directory; skipping recovery"
                );
            }
            Err(error) => return Err(io_error(&dir, error)),
        }
    }

    let mut referenced = HashSet::new();
    for journal in &journals {
        let mut settled = true;
        for root in &journal.roots {
            referenced.insert(root.stage.clone());
            if !is_current_root(config, &root.root) {
                tracing::warn!(
                    event = "purge.journal_foreign_root",
                    journal = %journal.txid,
                    root = %root.root.display(),
                    "Journal references a root that is not configured now; leaving it untouched"
                );
                settled = false;
                continue;
            }
            let result = match journal.state {
                JournalState::Staging => rollback_stage_path(&root.root, &root.stage),
                JournalState::Committed => remove_stage(&root.stage),
            };
            if let Err(error) = result {
                settled = false;
                tracing::warn!(
                    event = "purge.recovery_failed",
                    journal = %journal.txid,
                    root = %root.root.display(),
                    error = %error,
                    "Purge recovery failed; the journal is kept for the next attempt"
                );
            }
        }
        if settled {
            let path = dir.join(format!("{}.json", journal.txid));
            if let Err(error) = fs::remove_file(&path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(
                    event = "purge.journal_remove_failed",
                    journal = %path.display(),
                    error = %error,
                    "Purge recovery could not remove its journal"
                );
            }
        }
    }

    // Stages with no journal (a committed purge whose journal was already
    // removed, or a corrupt journal) are committed residue. An unreadable root
    // is skipped: recovery is best-effort cleanup and must not block commands
    // that do not touch it (a purge itself validates its roots and fails
    // closed).
    for root in [&config.general.store_dir, &config.general.source_dir] {
        if !root.exists() {
            continue;
        }
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                ) =>
            {
                tracing::warn!(
                    event = "purge.recovery_skipped",
                    root = %root.display(),
                    error = %error,
                    "Cannot inspect a cache root; skipping purge recovery for it"
                );
                continue;
            }
            Err(error) => return Err(io_error(root, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error(root, error))?;
            let path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(STAGE_PREFIX)
                && !referenced.contains(&path)
            {
                let _ = remove_stage(&path);
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------
// Keep set
// ----------------------------------------------------------------------

/// Verified pinned inputs referenced by an execution plan.
pub(crate) fn keep_entries(
    config: &GlobalConfig,
    plan: &BuildExecutionPlan,
) -> Result<Vec<(PathBuf, String)>> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for task in plan.build_set() {
        let Some(path) = plan.plan_path_for_task(task) else {
            continue;
        };
        if !seen.insert(path.to_path_buf()) {
            continue;
        }
        let manifest = PlanManifest::from_file(path).map_err(|error| {
            WrightError::context(format!("read plan {}", path.display()), error)
        })?;
        entries.extend(pinned_cache_entries(config, &manifest));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);
    Ok(entries)
}

pub(crate) fn keep_entries_for_manifest(
    config: &GlobalConfig,
    manifest: &PlanManifest,
) -> Vec<(PathBuf, String)> {
    pinned_cache_entries(config, manifest)
}

/// Verified pinned inputs for a set of plan names, resolved through the plan
/// index. Used by `upgrade`'s preview, which has names rather than an execution
/// plan.
pub(crate) fn keep_entries_for_names(
    config: &GlobalConfig,
    names: &[String],
) -> Result<Vec<(PathBuf, String)>> {
    let plan_dirs = crate::resolve::plan_search_dirs(config);
    let index = wright_plan::discovery::PlanIndex::discover(&plan_dirs)?;
    let mut entries = Vec::new();
    for name in names {
        let Some(path) = index.path_for(name) else {
            continue;
        };
        let manifest = PlanManifest::from_file(path).map_err(|error| {
            WrightError::context(format!("read plan {}", path.display()), error)
        })?;
        entries.extend(pinned_cache_entries(config, &manifest));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.dedup_by(|a, b| a.0 == b.0);
    Ok(entries)
}

// ----------------------------------------------------------------------
// Journal
// ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalState {
    Staging,
    Committed,
}

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    version: u32,
    txid: String,
    state: JournalState,
    roots: Vec<JournalRoot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalRoot {
    root: PathBuf,
    stage: PathBuf,
}

fn journal_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("purge")
}

fn write_journal(plan: &PurgePlan, state: JournalState) -> Result<PathBuf> {
    let dir = &plan.journal_dir;
    fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
    restrict_permissions(dir);

    let final_path = dir.join(format!("{}.json", plan.txid));
    let tmp_path = dir.join(format!("{}.json.tmp", plan.txid));
    let journal = Journal {
        version: JOURNAL_VERSION,
        txid: plan.txid.clone(),
        state,
        roots: plan
            .roots
            .iter()
            .map(|root| JournalRoot {
                root: root.root.clone(),
                stage: root.stage.clone(),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&journal)
        .map_err(|error| WrightError::context("serialize purge journal", error))?;

    {
        let mut file = File::create(&tmp_path).map_err(|error| io_error(&tmp_path, error))?;
        file.write_all(&bytes)
            .map_err(|error| io_error(&tmp_path, error))?;
        file.sync_all()
            .map_err(|error| io_error(&tmp_path, error))?;
    }
    fs::rename(&tmp_path, &final_path).map_err(|error| io_error(&final_path, error))?;
    sync_dir(dir);
    Ok(final_path)
}

fn read_journal(path: &Path) -> Result<Journal> {
    let bytes = fs::read(path).map_err(|error| io_error(path, error))?;
    let journal: Journal = serde_json::from_slice(&bytes).map_err(|error| {
        WrightError::context(format!("parse purge journal {}", path.display()), error)
    })?;
    if journal.version != JOURNAL_VERSION {
        return Err(WrightError::ValidationError(format!(
            "unsupported purge journal version {} in {}",
            journal.version,
            path.display()
        )));
    }
    Ok(journal)
}

// ----------------------------------------------------------------------
// Staging
// ----------------------------------------------------------------------

fn stage_root(root: &RootPlan) -> Result<()> {
    fs::create_dir_all(&root.stage).map_err(|error| io_error(&root.stage, error))?;
    restrict_permissions(&root.stage);
    for rel in &root.entries {
        stage_path(&root.root, &root.stage, &root.root.join(rel))?;
    }
    sync_dir(&root.stage);
    sync_dir(&root.root);
    Ok(())
}

fn stage_path(root: &Path, stage: &Path, source: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(source) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(source, error)),
    };

    if meta.is_dir() && !meta.file_type().is_symlink() {
        let mut children = Vec::new();
        for entry in fs::read_dir(source).map_err(|error| io_error(source, error))? {
            let entry = entry.map_err(|error| io_error(source, error))?;
            children.push(entry.path());
        }
        for child in children {
            stage_path(root, stage, &child)?;
        }
        return Ok(());
    }

    let rel = source.strip_prefix(root).map_err(|_| {
        WrightError::ValidationError(format!(
            "purge entry {} is not under {}",
            source.display(),
            root.display()
        ))
    })?;
    let dest = stage.join(rel);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    fs::rename(source, &dest).map_err(|error| io_error(source, error))
}

/// Restore a staged root. Current state wins: if the destination already
/// exists (another process wrote it while the purge was interrupted), the
/// staged copy is discarded rather than overwriting it.
fn rollback_stage(root: &RootPlan) -> Result<()> {
    rollback_stage_path(&root.root, &root.stage)
}

fn rollback_stage_path(root: &Path, stage: &Path) -> Result<()> {
    if !stage.exists() {
        return Ok(());
    }
    for item in walkdir::WalkDir::new(stage).into_iter().flatten() {
        if item.file_type().is_dir() {
            continue;
        }
        let path = item.path();
        let rel = path.strip_prefix(stage).map_err(|_| {
            WrightError::ValidationError(format!(
                "staged purge entry {} escapes {}",
                path.display(),
                stage.display()
            ))
        })?;
        let dest = root.join(rel);
        if dest.exists() {
            let _ = fs::remove_file(path);
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::rename(path, &dest).map_err(|error| io_error(path, error))?;
    }
    remove_stage(stage)
}

fn finalize_stage(root: &RootPlan) -> Result<()> {
    remove_stage(&root.stage)?;
    remove_empty_dirs(&root.root);
    Ok(())
}

fn remove_stage(stage: &Path) -> Result<()> {
    match fs::remove_dir_all(stage) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(stage, error)),
    }
}

fn remove_empty_dirs(root: &Path) {
    if !root.exists() {
        return;
    }
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_dir())
        .map(|entry| entry.path().to_path_buf())
        .collect();
    dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for dir in dirs {
        if dir
            .file_name()
            .map(|name| name.to_string_lossy().starts_with(STAGE_PREFIX))
            .unwrap_or(false)
        {
            continue;
        }
        let _ = fs::remove_dir(&dir);
    }
}

fn sync_dir(path: &Path) {
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn validate_layout(config: &GlobalConfig, db_path: &Path) -> Result<()> {
    config
        .validate_storage_layout(db_path)
        .map_err(|error| WrightError::ValidationError(error.to_string()))
}

fn is_current_root(config: &GlobalConfig, root: &Path) -> bool {
    root == config.general.source_dir.as_path() || root == config.general.store_dir.as_path()
}

// ----------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(root: &Path) -> GlobalConfig {
        let mut config = GlobalConfig::default();
        config.general.source_dir = root.join("sources");
        config.general.store_dir = root.join("store");
        config.general.parts_dir = root.join("parts");
        config.general.ledger_dir = root.join("ledger");
        config.general.logs_dir = root.join("logs");
        config.build.forge_dir = root.join("forge");
        config
    }

    fn sha256(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn keeps_only_verified_pinned_inputs() {
        let root = tempfile::tempdir().unwrap();
        let config = config_in(root.path());
        fs::create_dir_all(&config.general.source_dir).unwrap();

        let pinned = config.general.source_dir.join("pinned.tar.gz");
        fs::write(&pinned, b"pinned bytes").unwrap();
        let corrupt = config.general.source_dir.join("corrupt.tar.gz");
        fs::write(&corrupt, b"tampered").unwrap();
        let git = config.general.source_dir.join("repo-main-deadbeef.tar.zst");
        fs::write(&git, b"snapshot").unwrap();

        let keep = vec![
            (pinned.clone(), sha256(b"pinned bytes")),
            (corrupt.clone(), sha256(b"expected different bytes")),
        ];
        let plan = plan(
            &config,
            &root.path().join("wright.db"),
            &keep,
            &config.general.parts_dir,
        )
        .unwrap();

        assert_eq!(plan.paths, 2, "corrupt pinned + git snapshot are purged");
        assert_eq!(plan.kept, 1, "verified pinned input is kept");
        assert!(plan.reclaimed() > 0);
        // Store has no entries, so only the source root is present.
        assert_eq!(plan.roots.len(), 1);
        assert_eq!(plan.roots[0].root, config.general.source_dir);
    }

    #[test]
    fn store_is_staged_before_source() {
        let root = tempfile::tempdir().unwrap();
        let config = config_in(root.path());
        fs::create_dir_all(&config.general.source_dir).unwrap();
        fs::create_dir_all(&config.general.store_dir).unwrap();
        fs::write(config.general.source_dir.join("src.tar.gz"), b"source").unwrap();
        fs::write(config.general.store_dir.join("fp-part.part"), b"store").unwrap();

        let plan = plan(
            &config,
            &root.path().join("wright.db"),
            &[],
            &config.general.parts_dir,
        )
        .unwrap();
        assert_eq!(plan.roots.len(), 2);
        assert_eq!(plan.roots[0].root, config.general.store_dir);
        assert_eq!(plan.roots[1].root, config.general.source_dir);
    }

    #[test]
    fn rejects_overlapping_storage_layouts() {
        let root = tempfile::tempdir().unwrap();
        let mut config = config_in(root.path());
        config.general.source_dir = root.path().join("cache");
        config.general.parts_dir = root.path().join("cache/parts");
        fs::create_dir_all(&config.general.source_dir).unwrap();

        let error = plan(
            &config,
            &root.path().join("wright.db"),
            &[],
            &config.general.parts_dir,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("storage layout conflict"));
    }

    #[tokio::test]
    async fn execute_stages_and_finalizes_both_roots() {
        let root = tempfile::tempdir().unwrap();
        let config = config_in(root.path());
        fs::create_dir_all(&config.general.source_dir).unwrap();
        fs::create_dir_all(&config.general.store_dir).unwrap();
        fs::create_dir_all(&config.general.parts_dir).unwrap();
        fs::write(config.general.source_dir.join("src.tar.gz"), b"source").unwrap();
        fs::write(config.general.store_dir.join("fp-part.part"), b"store").unwrap();
        let archive = config.general.parts_dir.join("keepme.wright.tar.zst");
        fs::write(&archive, b"inventory").unwrap();
        let parts = config.general.parts_dir.clone();

        let plan = plan(&config, &root.path().join("wright.db"), &[], &parts).unwrap();
        execute_silent(&plan).await.unwrap();

        assert!(!config.general.source_dir.join("src.tar.gz").exists());
        assert!(!config.general.store_dir.join("fp-part.part").exists());
        assert!(
            !plan.journal_dir.exists() || fs::read_dir(&plan.journal_dir).unwrap().next().is_none()
        );
        assert!(archive.exists(), "parts_dir is inventory, never purged");
    }

    #[tokio::test]
    async fn recovered_staging_is_rolled_back() {
        let root = tempfile::tempdir().unwrap();
        let config = config_in(root.path());
        fs::create_dir_all(&config.general.source_dir).unwrap();
        let source = config.general.source_dir.join("src.tar.gz");
        fs::write(&source, b"source").unwrap();

        let db = root.path().join("wright.db");
        let txid = "staging-test".to_string();
        let stage = config
            .general
            .source_dir
            .join(format!("{STAGE_PREFIX}{txid}"));
        fs::create_dir_all(&stage).unwrap();
        fs::rename(&source, stage.join("src.tar.gz")).unwrap();

        let journal = Journal {
            version: JOURNAL_VERSION,
            txid: txid.clone(),
            state: JournalState::Staging,
            roots: vec![JournalRoot {
                root: config.general.source_dir.clone(),
                stage: stage.clone(),
            }],
        };
        let dir = journal_dir(&db);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{txid}.json")),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        recover(&config, &db).unwrap();

        assert!(source.exists(), "an uncommitted purge is restored");
        assert!(!stage.exists());
        assert!(!dir.join(format!("{txid}.json")).exists());
    }

    #[tokio::test]
    async fn recovered_commit_is_finalized() {
        let root = tempfile::tempdir().unwrap();
        let config = config_in(root.path());
        fs::create_dir_all(&config.general.source_dir).unwrap();

        let db = root.path().join("wright.db");
        let txid = "committed-test".to_string();
        let stage = config
            .general
            .source_dir
            .join(format!("{STAGE_PREFIX}{txid}"));
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("src.tar.gz"), b"source").unwrap();

        let journal = Journal {
            version: JOURNAL_VERSION,
            txid: txid.clone(),
            state: JournalState::Committed,
            roots: vec![JournalRoot {
                root: config.general.source_dir.clone(),
                stage: stage.clone(),
            }],
        };
        let dir = journal_dir(&db);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{txid}.json")),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();

        recover(&config, &db).unwrap();

        assert!(!stage.exists(), "a committed purge is deleted");
        assert!(!config.general.source_dir.join("src.tar.gz").exists());
    }

    #[tokio::test]
    async fn plan_entries_stages_nested_files_and_preserves_paths() {
        let root = tempfile::tempdir().unwrap();
        let config = config_in(root.path());
        let source_dir = &config.general.source_dir;
        std::fs::create_dir_all(source_dir.join("sub")).unwrap();
        let selected = source_dir.join("sub/selected.bin");
        let kept = source_dir.join("sub/kept.bin");
        std::fs::write(&selected, b"selected").unwrap();
        std::fs::write(&kept, b"kept").unwrap();

        let plan = plan_entries(
            &config,
            &root.path().join("wright.db"),
            &config.general.parts_dir,
            vec![(source_dir.clone(), vec![selected.clone()])],
        )
        .unwrap();
        execute_silent(&plan).await.unwrap();

        assert!(!selected.exists(), "the selected nested file is purged");
        assert!(kept.exists(), "unselected siblings survive");
    }

    #[test]
    fn reclaimed_bytes_ignores_hard_links_into_parts() {
        let root = tempfile::tempdir().unwrap();
        let parts = root.path().join("parts");
        let store = root.path().join("store");
        fs::create_dir_all(&parts).unwrap();
        fs::create_dir_all(&store).unwrap();

        let archive = parts.join("pkg.wright.tar.zst");
        fs::write(&archive, b"payload").unwrap();
        let linked = store.join("fp-pkg.part");
        fs::hard_link(&archive, &linked).unwrap();
        let orphan = store.join("fp-orphan.part");
        fs::write(&orphan, b"orphan-payload").unwrap();

        let bytes = reclaimed_bytes(&[linked.clone(), orphan.clone()], &parts);
        assert_eq!(bytes, b"orphan-payload".len() as u64);

        fs::remove_file(&linked).unwrap();
        let bytes = reclaimed_bytes(std::slice::from_ref(&archive), &parts);
        assert_eq!(bytes, 0, "an inode still live in parts_dir frees nothing");
    }
}
