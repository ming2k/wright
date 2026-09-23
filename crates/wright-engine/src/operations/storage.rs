//! Read-only storage accounting for every location Wright owns (ADR-0043).
//!
//! `storage` measures; it never deletes. Its purpose is to make retention
//! decisions against real bytes instead of estimates: each row names the
//! `clean` flag (or diagnostic command) that would reclaim it. The companion
//! discipline — a location with no `storage` row gets no deletion flag — is what
//! keeps the reclamation surface honest.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::GlobalConfig;
use crate::error::Result;
use wright_state::database::ReadOnlyDb;

/// One measured location: the directories, cache files, ledger, and database
/// file Wright accumulates over a machine's lifetime.
#[derive(Debug, Serialize)]
pub struct StorageEntry {
    /// Short label (`parts`, `store`, `sources`, ...).
    pub location: String,
    /// Absolute path measured.
    pub path: String,
    /// Apparent bytes on disk (sum of file sizes; hard links counted once).
    pub bytes: u64,
    /// Number of files counted.
    pub entries: u64,
    /// Bytes a reclamation command would actually free, when that is
    /// computable without the flag's own parameters. `None` means the rule is
    /// parameterised (age / keep-N) and needs an explicit argument.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reclaimable_bytes: Option<u64>,
    /// How this location is reclaimed — the exact command that owns it.
    pub rule: String,
}

/// Full accounting report for one machine.
#[derive(Debug, Serialize)]
pub struct StorageReport {
    pub locations: Vec<StorageEntry>,
    /// Sum of every location's apparent bytes.
    pub total_bytes: u64,
    /// Deployed footprint recorded in the registry (`files.file_size`); `None`
    /// when the registry could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployed_bytes: Option<u64>,
    /// Deployed file count; `None` when the registry could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployed_files: Option<u64>,
}

/// Measure every location Wright owns and print the report.
///
/// The database is read best-effort: when it cannot be opened (the exact
/// situation ADR-0043 makes a first-class case) the filesystem rows are still
/// reported and the deployed-footprint row is omitted.
pub async fn execute_storage(
    config: &GlobalConfig,
    db_path: &Path,
    db: Option<&ReadOnlyDb>,
    json: bool,
) -> Result<()> {
    let report = build_report(config, db_path, db).await?;

    if json {
        return super::print_json(&report);
    }

    crate::cli_output!(
        "{:<10} {:>12} {:>8}  {}",
        "LOCATION",
        "BYTES",
        "FILES",
        "RULE"
    );
    for entry in &report.locations {
        crate::cli_output!(
            "{:<10} {:>12} {:>8}  {}",
            entry.location,
            crate::util::display::format_bytes(entry.bytes),
            entry.entries,
            entry.rule
        );
    }
    crate::cli_output!(
        "{:<10} {:>12}",
        "total",
        crate::util::display::format_bytes(report.total_bytes),
    );
    if let Some(deployed) = report.deployed_bytes {
        crate::cli_output!(
            "{:<10} {:>12}  ({} files, deployed footprint)",
            "deployed",
            crate::util::display::format_bytes(deployed),
            report.deployed_files.unwrap_or(0),
        );
    } else {
        crate::cli_warn!(
            "registry at {} is unreadable; deployed footprint omitted (run `wright doctor --repair`)",
            db_path.display()
        );
    }
    Ok(())
}

/// Build the report without printing. Kept separate so tests can assert on the
/// numbers rather than on formatted output.
pub async fn build_report(
    config: &GlobalConfig,
    db_path: &Path,
    db: Option<&ReadOnlyDb>,
) -> Result<StorageReport> {
    let mut locations = Vec::new();

    // Build workspaces — fully reclaimable by `clean`.
    let (bytes, files) = crate::ledger::dir_stats(&config.build.forge_dir);
    locations.push(StorageEntry {
        location: "forge".to_string(),
        path: config.build.forge_dir.display().to_string(),
        bytes,
        entries: files,
        reclaimable_bytes: Some(bytes),
        rule: "clean (build workspaces)".to_string(),
    });

    // Part archives — always needed; superseded versions are stale.
    let archive = archive_usage(&config.general.parts_dir)?;
    locations.push(StorageEntry {
        location: "parts".to_string(),
        path: config.general.parts_dir.display().to_string(),
        bytes: archive.bytes,
        entries: archive.count,
        reclaimable_bytes: Some(archive.stale_bytes),
        rule: "clean --stale (superseded versions)".to_string(),
    });

    // CAS store — a pure rebuild cache; entries not linked to a live archive
    // are orphaned copies that free real space when unlinked.
    let store = store_reclaimable(&config.general.store_dir, &config.general.parts_dir);
    let (store_bytes, store_files) = crate::ledger::dir_stats(&config.general.store_dir);
    locations.push(StorageEntry {
        location: "store".to_string(),
        path: config.general.store_dir.display().to_string(),
        bytes: store_bytes,
        entries: store_files,
        reclaimable_bytes: Some(store.bytes),
        rule: "clean --store (entries unlinked from any archive)".to_string(),
    });

    // Source cache — reclaimable only by age.
    let (source_bytes, source_files) = crate::ledger::dir_stats(&config.general.source_dir);
    locations.push(StorageEntry {
        location: "sources".to_string(),
        path: config.general.source_dir.display().to_string(),
        bytes: source_bytes,
        entries: source_files,
        reclaimable_bytes: None,
        rule: "clean --sources [--older-than-days N]".to_string(),
    });

    // Command logs — fully reclaimable by `clean --logs`.
    let (log_bytes, log_files) = crate::ledger::dir_stats(&config.general.logs_dir);
    locations.push(StorageEntry {
        location: "logs".to_string(),
        path: config.general.logs_dir.display().to_string(),
        bytes: log_bytes,
        entries: log_files,
        reclaimable_bytes: Some(log_bytes),
        rule: "clean --logs (command logs)".to_string(),
    });

    // Audit ledger — reclaimable only by rotation.
    let ledger_dir = crate::ledger::dir(config, Some(db_path));
    let (ledger_bytes, ledger_files) = crate::ledger::dir_stats(&ledger_dir);
    locations.push(StorageEntry {
        location: "ledger".to_string(),
        path: ledger_dir.display().to_string(),
        bytes: ledger_bytes,
        entries: ledger_files,
        reclaimable_bytes: None,
        rule: "clean --ledger [--keep-builds N]".to_string(),
    });

    // The database file plus its WAL sidecar.
    let db_bytes = sidecar_bytes(db_path);
    locations.push(StorageEntry {
        location: "database".to_string(),
        path: db_path.display().to_string(),
        bytes: db_bytes,
        entries: 1,
        reclaimable_bytes: None,
        rule: "doctor --snapshot / doctor --repair".to_string(),
    });

    let total_bytes: u64 = locations
        .iter()
        .map(|entry| entry.bytes)
        .fold(0u64, u64::saturating_add);

    let (deployed_bytes, deployed_files) = match db {
        Some(db) => match db.file_usage().await {
            Ok((bytes, files)) => (Some(bytes), Some(files)),
            Err(_) => (None, None),
        },
        None => (None, None),
    };

    Ok(StorageReport {
        locations,
        total_bytes,
        deployed_bytes,
        deployed_files,
    })
}

struct ArchiveUsage {
    bytes: u64,
    count: u64,
    stale_bytes: u64,
}

/// Total bytes and count of part archives, plus the bytes a `clean --stale`
/// run would free.
fn archive_usage(parts_dir: &Path) -> Result<ArchiveUsage> {
    let mut bytes = 0u64;
    let mut count = 0u64;
    if parts_dir.exists() {
        for entry in walkdir::WalkDir::new(parts_dir).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let is_archive = entry
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".wright.tar.zst"));
            if !is_archive {
                continue;
            }
            count += 1;
            bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        }
    }

    let stale_bytes = super::clean::stale_archives(parts_dir, &[])?
        .iter()
        .filter_map(|path| std::fs::symlink_metadata(path).ok())
        .map(|meta| meta.len())
        .sum();

    Ok(ArchiveUsage {
        bytes,
        count,
        stale_bytes,
    })
}

struct Reclaimable {
    bytes: u64,
}

/// Bytes a `clean --store` run would actually free.
///
/// A CAS entry is a hard link to the part archive it caches (or a copy on
/// cross-device fallback). Removing a still-linked entry frees nothing,
/// because the archive keeps the inode alive — so reclamation counts only
/// entries whose inode is shared with no live `parts_dir` archive. Reported
/// bytes are per *inode*: two orphaned entries hard-linked to each other
/// release their bytes once, not twice.
fn store_reclaimable(store_dir: &Path, parts_dir: &Path) -> Reclaimable {
    if !store_dir.exists() {
        return Reclaimable { bytes: 0 };
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
    let mut bytes = 0u64;
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
        if counted.insert(key) {
            bytes = bytes.saturating_add(meta.len());
        }
    }
    Reclaimable { bytes }
}

/// Bytes occupied by the database file plus its `-wal` sidecar.
fn sidecar_bytes(db_path: &Path) -> u64 {
    let mut total = std::fs::symlink_metadata(db_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let wal = path_with_suffix(db_path, "-wal");
    total += std::fs::symlink_metadata(&wal)
        .map(|meta| meta.len())
        .unwrap_or(0);
    total
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_reclaimable_counts_orphans_once_per_inode() {
        let tmp = tempfile::tempdir().unwrap();
        let parts = tmp.path().join("parts");
        let store = tmp.path().join("store");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::create_dir_all(&store).unwrap();

        // A live archive and its hard-linked CAS entry (counts as 0 freed).
        let archive = parts.join("demo-1.0.0.wright.tar.zst");
        std::fs::write(&archive, vec![0u8; 100]).unwrap();
        std::fs::hard_link(&archive, store.join("aaaaaaaaaaaaaaaa-demo.part")).unwrap();

        // An orphaned standalone cache copy (frees its bytes).
        std::fs::write(store.join("bbbbbbbbbbbbbbbb-demo.part"), vec![0u8; 40]).unwrap();

        let reclaimable = store_reclaimable(&store, &parts);
        assert_eq!(reclaimable.bytes, 40);
    }

    #[test]
    fn sidecar_bytes_sums_database_and_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("wright.db");
        std::fs::write(&db, vec![0u8; 10]).unwrap();
        std::fs::write(tmp.path().join("wright.db-wal"), vec![0u8; 5]).unwrap();
        assert_eq!(sidecar_bytes(&db), 15);
    }
}
