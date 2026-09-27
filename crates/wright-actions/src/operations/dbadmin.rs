//! Database maintenance: snapshot, restore, and registry rebuild (ADR-0043).
//!
//! `wright.db` is a derived index. These operations treat it that way —
//! snapshot it before a risky change, restore it from a snapshot, or rebuild
//! it from the inventory when it has been damaged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::GlobalConfig;
use crate::error::{Result, WrightError};
use wright_part::archive;
use wright_registry::database::{
    FileType, InstalledDb, Origin, RebuiltFile, RebuiltPart, RebuiltPlan,
};

/// SQLite's 16-byte file magic. Used to reject an obvious non-database before
/// it is copied over a live registry.
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Snapshot the live database to `output` (default: `<db>.backup`).
///
/// Uses `VACUUM INTO`, which writes a single consistent file and is immune to
/// the WAL hazard of copying `wright.db` by hand.
pub async fn execute_backup(
    db: &InstalledDb,
    db_path: &Path,
    output: Option<&Path>,
    json: bool,
) -> Result<()> {
    let target = output
        .map(PathBuf::from)
        .unwrap_or_else(|| default_backup_path(db_path));

    let target_str = target.to_string_lossy().into_owned();
    let target_for_job = target.clone();
    db.write(move |conn| {
        // VACUUM INTO refuses an existing target; a repeat backup overwrites.
        let _ = std::fs::remove_file(&target_for_job);
        conn.execute("VACUUM INTO ?1", [target_str.as_str()])
            .map_err(|e| wright_registry::StateError::context("failed to snapshot database", e))?;
        Ok(())
    })
    .await?;

    let bytes = std::fs::symlink_metadata(&target)
        .map(|meta| meta.len())
        .unwrap_or(0);

    if json {
        #[derive(Serialize)]
        struct BackupReport<'a> {
            source: &'a str,
            output: &'a str,
            bytes: u64,
        }
        return super::print_json(&BackupReport {
            source: &db_path.display().to_string(),
            output: &target.display().to_string(),
            bytes,
        });
    }
    crate::cli_action!(
        "Backed up",
        "{} -> {} ({})",
        db_path.display(),
        target.display(),
        crate::util::display::format_bytes(bytes)
    );
    Ok(())
}

/// Restore the database at `db_path` from `backup`.
///
/// The database must not be open: the caller holds the database lock. `-wal`
/// and `-shm` sidecars are removed so the restored main file is authoritative
/// and cannot be recombined with stale WAL frames from the pre-restore state.
pub fn execute_restore(backup: &Path, db_path: &Path) -> Result<()> {
    if !backup.exists() {
        return Err(WrightError::ValidationError(format!(
            "backup not found: {}",
            backup.display()
        )));
    }

    let header = read_header(backup)?;
    if header != SQLITE_MAGIC {
        return Err(WrightError::ValidationError(format!(
            "{} is not a SQLite database (bad header)",
            backup.display()
        )));
    }

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            WrightError::context(
                format!("failed to create database directory {}", parent.display()),
                e,
            )
        })?;
    }

    std::fs::copy(backup, db_path).map_err(|e| {
        WrightError::context(
            format!(
                "failed to restore {} -> {}",
                backup.display(),
                db_path.display()
            ),
            e,
        )
    })?;

    for suffix in ["-wal", "-shm"] {
        let sidecar = path_with_suffix(db_path, suffix);
        if sidecar.exists() {
            std::fs::remove_file(&sidecar).map_err(|e| {
                WrightError::context(format!("failed to remove {}", sidecar.display()), e)
            })?;
        }
    }

    crate::cli_action!("Restored", "{} -> {}", backup.display(), db_path.display());
    Ok(())
}

/// Rebuild the registry from archives in `parts_dir` (and, when
/// `include_store`, orphaned copies in `store_dir`).
pub async fn execute_reindex(
    config: &GlobalConfig,
    root_dir: &Path,
    db: &InstalledDb,
    include_store: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let archives = collect_archives(
        &config.general.parts_dir,
        include_store,
        &config.general.store_dir,
    )?;
    if archives.is_empty() {
        return Err(WrightError::ValidationError(format!(
            "no archives found under {}; nothing to rebuild from",
            config.general.parts_dir.display()
        )));
    }

    let plans = build_plans(&archives, root_dir)?;

    let part_count: usize = plans.iter().map(|plan| plan.parts.len()).sum();
    let file_count: usize = plans
        .iter()
        .flat_map(|plan| plan.parts.iter())
        .map(|part| part.files.len())
        .sum();

    if dry_run {
        if json {
            #[derive(Serialize)]
            struct ReindexPreview<'a> {
                plans: usize,
                parts: usize,
                files: usize,
                source: &'a str,
            }
            return super::print_json(&ReindexPreview {
                plans: plans.len(),
                parts: part_count,
                files: file_count,
                source: &config.general.parts_dir.display().to_string(),
            });
        }
        crate::cli_action!(
            "Dry-run",
            "would rebuild registry from {} plan(s), {} part(s), {} file(s)",
            plans.len(),
            part_count,
            file_count
        );
        for plan in &plans {
            for part in &plan.parts {
                crate::cli_output!("  {}:{}", plan.name, part.name);
            }
        }
        return Ok(());
    }

    // Preserve what the current registry still knows about origins; a damaged
    // database may not answer, in which case every part rebuilds as `manual`.
    let preserve_origins: HashMap<String, Origin> = db
        .list_parts()
        .await
        .map(|parts| {
            parts
                .into_iter()
                .map(|part| (part.name, part.origin))
                .collect()
        })
        .unwrap_or_default();

    let summary = db.rebuild_registry(plans, preserve_origins).await?;

    if json {
        #[derive(Serialize)]
        struct ReindexReport {
            plans: usize,
            parts: usize,
            files: usize,
            dependencies: usize,
        }
        return super::print_json(&ReindexReport {
            plans: summary.plans,
            parts: summary.parts,
            files: summary.files,
            dependencies: summary.dependencies,
        });
    }
    crate::cli_action!(
        "Rebuilt",
        "registry: {} plan(s), {} part(s), {} file(s), {} dependency edge(s)",
        summary.plans,
        summary.parts,
        summary.files,
        summary.dependencies
    );
    crate::cli_output!(
        "Note: history was preserved; installed_at timestamps and per-part build provenance are not recoverable from archives."
    );
    Ok(())
}

fn default_backup_path(db_path: &Path) -> PathBuf {
    let mut name = db_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wright.db".to_string());
    name.push_str(".backup");
    db_path.with_file_name(name)
}

fn read_header(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|e| WrightError::context(format!("failed to open {}", path.display()), e))?;
    let mut header = vec![0u8; SQLITE_MAGIC.len()];
    file.read_exact(&mut header).map_err(|_| {
        WrightError::ValidationError(format!(
            "{} is too small to be a SQLite database",
            path.display()
        ))
    })?;
    Ok(header)
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push_str(suffix);
    path.with_file_name(name)
}

/// One archive candidate plus its parsed metadata.
struct ArchiveCandidate {
    partinfo: archive::PartInfo,
    files: Vec<String>,
    part_hash: Option<String>,
}

/// Collect every readable archive under `parts_dir`, and — when requested —
/// any `store_dir` entry that is not already represented. Parts take
/// precedence because their placement is the canonical inventory; the store is
/// a fingerprint-named cache whose namespace is shared across plans and eras.
fn collect_archives(
    parts_dir: &Path,
    include_store: bool,
    store_dir: &Path,
) -> Result<Vec<ArchiveCandidate>> {
    let mut candidates = scan_dir(parts_dir)?;
    let mut seen: std::collections::HashSet<(String, String, String, u32, u32)> =
        candidates.iter().map(|c| part_key(&c.partinfo)).collect();

    if include_store {
        for candidate in scan_dir(store_dir)? {
            if seen.insert(part_key(&candidate.partinfo)) {
                candidates.push(candidate);
            }
        }
    }
    Ok(candidates)
}

fn part_key(info: &archive::PartInfo) -> (String, String, String, u32, u32) {
    (
        info.plan.name.clone(),
        info.name.clone(),
        info.plan.version.clone(),
        info.plan.release,
        info.plan.epoch,
    )
}

fn scan_dir(dir: &Path) -> Result<Vec<ArchiveCandidate>> {
    let mut candidates = Vec::new();
    if !dir.exists() {
        return Ok(candidates);
    }
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let is_archive = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".wright.tar.zst") || name.ends_with(".part"));
        if !is_archive {
            continue;
        }
        match archive::read_archive_meta(path) {
            Ok(meta) => {
                let part_hash = wright_part::compression::archive_sha256(path).ok();
                candidates.push(ArchiveCandidate {
                    partinfo: meta.partinfo,
                    files: meta.files,
                    part_hash,
                });
            }
            Err(e) => {
                crate::cli_warn!("skipping unreadable archive {}: {}", path.display(), e);
            }
        }
    }
    Ok(candidates)
}

/// Group archive candidates into plans with their outputs, statting each
/// recorded path against the live root so the rebuilt rows carry real types
/// and sizes.
fn build_plans(candidates: &[ArchiveCandidate], root_dir: &Path) -> Result<Vec<RebuiltPlan>> {
    let mut by_plan: HashMap<String, RebuiltPlan> = HashMap::new();

    for candidate in candidates {
        let info = &candidate.partinfo;
        let files = candidate
            .files
            .iter()
            .map(|path| stat_file(root_dir, path))
            .collect::<Vec<_>>();

        let part = RebuiltPart {
            name: info.name.clone(),
            part_hash: candidate.part_hash.clone(),
            origin: Origin::Manual,
            files,
            dependencies: info.runtime_deps.clone(),
            conflicts: info.conflicts.clone(),
            replaces: info.replaces.clone(),
        };

        by_plan
            .entry(info.plan.name.clone())
            .and_modify(|plan| plan.parts.push(part.clone()))
            .or_insert_with(|| {
                let provenance = info.provenance.as_ref();
                RebuiltPlan {
                    name: info.plan.name.clone(),
                    version: info.plan.version.clone(),
                    release: info.plan.release,
                    epoch: info.plan.epoch,
                    arch: info.plan.arch.clone(),
                    plan_checksum: provenance.and_then(|p| p.plan_checksum.clone()),
                    source_checksums: provenance
                        .map(|p| p.source_checksums.clone())
                        .unwrap_or_default(),
                    wright_version: provenance.map(|p| p.wright_version.clone()),
                    isolation: provenance.map(|p| p.isolation.clone()),
                    parts: vec![part],
                }
            });
    }

    Ok(by_plan.into_values().collect())
}

fn stat_file(root_dir: &Path, recorded: &str) -> RebuiltFile {
    let absolute = root_dir.join(recorded.trim_start_matches('/'));
    match std::fs::symlink_metadata(&absolute) {
        Ok(meta) => {
            let file_type = meta.file_type();
            let (kind, size) = if file_type.is_symlink() {
                (FileType::Symlink, None)
            } else if file_type.is_dir() {
                (FileType::Directory, None)
            } else {
                (FileType::File, Some(meta.len() as i64))
            };
            RebuiltFile {
                path: recorded.to_string(),
                file_type: kind,
                file_size: size,
            }
        }
        Err(_) => RebuiltFile {
            path: recorded.to_string(),
            file_type: FileType::File,
            file_size: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backup_path_appends_suffix() {
        assert_eq!(
            default_backup_path(Path::new("/var/lib/wright/wright.db")),
            PathBuf::from("/var/lib/wright/wright.db.backup")
        );
    }

    #[test]
    fn restore_rejects_non_database_file() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("bogus.bak");
        std::fs::write(&bogus, b"not a sqlite database at all").unwrap();
        let db = tmp.path().join("wright.db");
        let err = execute_restore(&bogus, &db).unwrap_err();
        assert!(err.to_string().contains("not a SQLite database"));
        assert!(
            !db.exists(),
            "a rejected restore must not create the target"
        );
    }

    #[test]
    fn restore_rejects_truncated_file_with_clear_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tiny = tmp.path().join("tiny.bak");
        std::fs::write(&tiny, b"short").unwrap();
        let db = tmp.path().join("wright.db");
        let err = execute_restore(&tiny, &db).unwrap_err();
        assert!(
            err.to_string()
                .contains("too small to be a SQLite database"),
            "got: {err}"
        );
    }

    #[test]
    fn restore_copies_database_and_drops_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp.path().join("wright.db.bak");
        let mut contents = SQLITE_MAGIC.to_vec();
        contents.extend_from_slice(b"payload");
        std::fs::write(&backup, &contents).unwrap();

        let db = tmp.path().join("wright.db");
        std::fs::write(&db, b"old").unwrap();
        std::fs::write(tmp.path().join("wright.db-wal"), b"stale wal").unwrap();

        execute_restore(&backup, &db).unwrap();
        assert_eq!(std::fs::read(&db).unwrap(), contents);
        assert!(!tmp.path().join("wright.db-wal").exists());
    }

    #[test]
    fn stat_file_reports_type_and_size() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("usr/bin")).unwrap();
        std::fs::write(tmp.path().join("usr/bin/demo"), vec![0u8; 7]).unwrap();

        let file = stat_file(tmp.path(), "/usr/bin/demo");
        assert_eq!(file.file_type, FileType::File);
        assert_eq!(file.file_size, Some(7));

        let dir = stat_file(tmp.path(), "/usr/bin");
        assert_eq!(dir.file_type, FileType::Directory);

        let missing = stat_file(tmp.path(), "/usr/bin/nope");
        assert_eq!(missing.file_size, None);
    }
}
