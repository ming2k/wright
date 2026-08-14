//! File-backed plan-source snapshot ledger.
//!
//! Layout (ADR-0041):
//!
//! ```text
//! <ledger_dir>/<plan>/snapshots/<yyyymmddThhmmssZ>-<sha256>.toml
//! <ledger_dir>/_detached/<yyyymmddThhmmssZ>-<sha256>.toml
//! ```
//!
//! Each file holds the exact plan.toml text that produced one registered
//! part generation — the same bytes the archive embeds as `.PLANSRC`.
//! Files are named by recording time so `ls` reads as a plan's source
//! history and `diff` answers "what drifted" with no tooling; the trailing
//! checksum ties a file back to `plans.plan_checksum` in the database.
//!
//! Snapshots are append-only audit data: a new file is written only when
//! the checksum differs from every snapshot already recorded for the plan,
//! and nothing is ever deleted. Rows migrated from the retired
//! `plan_snapshots` table whose plan no longer exists land in `_detached/`.

use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};

/// Directory holding snapshots whose owning plan row is gone (migrated
/// legacy rows only — runtime writes always know their plan).
const DETACHED_DIR: &str = "_detached";

/// Record a plan-source snapshot unless an identical one (same checksum)
/// already exists for the plan. Returns the path written, or `None` when
/// the snapshot was already on file. `recorded_at` supplies the timestamp
/// for migrated legacy rows; `None` stamps the current time.
pub fn record_plan_snapshot(
    ledger_dir: &Path,
    plan: &str,
    checksum: &str,
    source: &str,
    recorded_at: Option<&str>,
) -> Result<Option<PathBuf>> {
    let dir = snapshot_dir(ledger_dir, plan);
    if find_snapshot(&dir, checksum)?.is_some() {
        return Ok(None);
    }
    std::fs::create_dir_all(&dir).map_err(|e| {
        WrightError::context(
            format!("failed to create snapshot dir {}", dir.display()),
            e,
        )
    })?;

    let file_name = format!("{}-{}.toml", snapshot_timestamp(recorded_at), checksum);
    let path = dir.join(&file_name);
    // Write-then-rename so a crash mid-write never leaves a truncated
    // snapshot behind under a final name.
    let tmp = dir.join(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, source)
        .map_err(|e| WrightError::context(format!("failed to write {}", tmp.display()), e))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| WrightError::context(format!("failed to commit {}", path.display()), e))?;
    Ok(Some(path))
}

/// Fetch a recorded snapshot by plan and checksum. `None` when no part
/// carrying that checksum was ever registered (or the ledger is
/// unreadable) — snapshot lookup is advisory and degrades quietly.
pub fn plan_snapshot_source(ledger_dir: &Path, plan: &str, checksum: &str) -> Option<String> {
    let dir = snapshot_dir(ledger_dir, plan);
    let path = find_snapshot(&dir, checksum).ok()??;
    std::fs::read_to_string(path).ok()
}

fn snapshot_dir(ledger_dir: &Path, plan: &str) -> PathBuf {
    ledger_dir.join(plan).join("snapshots")
}

/// Locate the snapshot file for `checksum` in `dir` by its
/// `-<checksum>.toml` suffix.
fn find_snapshot(dir: &Path, checksum: &str) -> Result<Option<PathBuf>> {
    let suffix = format!("-{}.toml", checksum);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(WrightError::context(
                format!("failed to read snapshot dir {}", dir.display()),
                e,
            ));
        }
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(&suffix) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// Snapshot filename timestamp: `yyyymmddThhmmssZ`, sortable as plain text.
/// Legacy `recorded_at` values arrive as SQLite's `YYYY-MM-DD HH:MM:SS`
/// (UTC) and are reformatted; anything unparseable falls back to now.
fn snapshot_timestamp(recorded_at: Option<&str>) -> String {
    recorded_at
        .and_then(|raw| {
            chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| dt.format("%Y%m%dT%H%M%SZ").to_string())
        })
        .unwrap_or_else(|| chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string())
}

/// One-time export of the legacy `plan_snapshots` table into the ledger,
/// run before the migration that drops the table. Idempotent: existing
/// files are kept (same checksum = same content), so a partially completed
/// export simply resumes on the next open. Returns the number of rows
/// found; zero when the table never existed.
pub(crate) async fn export_legacy_plan_snapshots(
    pool: &sqlx::SqlitePool,
    ledger_dir: &Path,
) -> Result<usize> {
    use sqlx::Row;

    let exists: bool = sqlx::query(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'plan_snapshots'",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| WrightError::context("failed to inspect legacy snapshot table", e))?
    .get::<i64, _>(0)
        > 0;
    if !exists {
        return Ok(0);
    }

    // Snapshots outlive their plan row by design (the table was a retained
    // ledger), hence the LEFT JOIN: nameless rows export into `_detached/`.
    let rows = sqlx::query(
        "SELECT ps.checksum, ps.source, ps.recorded_at, pl.name AS plan_name
         FROM plan_snapshots ps
         LEFT JOIN plans pl ON pl.plan_checksum = ps.checksum",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| WrightError::context("failed to read legacy plan snapshots", e))?;

    for row in &rows {
        let checksum: &str = row
            .try_get("checksum")
            .map_err(|e| WrightError::context("malformed legacy snapshot row", e))?;
        let source: &str = row
            .try_get("source")
            .map_err(|e| WrightError::context("malformed legacy snapshot row", e))?;
        let recorded_at: Option<String> = row
            .try_get("recorded_at")
            .map_err(|e| WrightError::context("malformed legacy snapshot row", e))?;
        let plan_name: Option<String> = row
            .try_get("plan_name")
            .map_err(|e| WrightError::context("malformed legacy snapshot row", e))?;

        match plan_name {
            Some(name) => {
                record_plan_snapshot(ledger_dir, &name, checksum, source, recorded_at.as_deref())?;
            }
            None => {
                record_detached_snapshot(ledger_dir, checksum, source, recorded_at.as_deref())?;
            }
        }
    }

    if !rows.is_empty() {
        tracing::info!(
            "exported {} legacy plan snapshot(s) into {}",
            rows.len(),
            ledger_dir.display()
        );
    }
    Ok(rows.len())
}

fn record_detached_snapshot(
    ledger_dir: &Path,
    checksum: &str,
    source: &str,
    recorded_at: Option<&str>,
) -> Result<()> {
    let dir = ledger_dir.join(DETACHED_DIR);
    if find_snapshot(&dir, checksum)?.is_some() {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| {
        WrightError::context(
            format!("failed to create snapshot dir {}", dir.display()),
            e,
        )
    })?;
    let path = dir.join(format!(
        "{}-{}.toml",
        snapshot_timestamp(recorded_at),
        checksum
    ));
    std::fs::write(&path, source)
        .map_err(|e| WrightError::context(format!("failed to write {}", path.display()), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let written =
            record_plan_snapshot(dir.path(), "demo", "abc123", "name = \"demo\"\n", None).unwrap();
        assert!(written.is_some());
        assert_eq!(
            plan_snapshot_source(dir.path(), "demo", "abc123").as_deref(),
            Some("name = \"demo\"\n")
        );
    }

    #[test]
    fn same_checksum_is_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        record_plan_snapshot(dir.path(), "demo", "abc123", "v1", None).unwrap();
        let second = record_plan_snapshot(dir.path(), "demo", "abc123", "v1", None).unwrap();
        assert!(second.is_none(), "identical checksum must dedup");
        let files = std::fs::read_dir(dir.path().join("demo/snapshots"))
            .unwrap()
            .count();
        assert_eq!(files, 1);
    }

    #[test]
    fn changed_checksum_appends_new_file() {
        let dir = tempfile::tempdir().unwrap();
        record_plan_snapshot(dir.path(), "demo", "aaa", "v1", None).unwrap();
        record_plan_snapshot(dir.path(), "demo", "bbb", "v2", None).unwrap();
        assert_eq!(
            plan_snapshot_source(dir.path(), "demo", "aaa").as_deref(),
            Some("v1")
        );
        assert_eq!(
            plan_snapshot_source(dir.path(), "demo", "bbb").as_deref(),
            Some("v2")
        );
    }

    #[test]
    fn lookup_miss_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(plan_snapshot_source(dir.path(), "demo", "nosuch").is_none());
    }

    #[test]
    fn legacy_recorded_at_drives_filename() {
        let dir = tempfile::tempdir().unwrap();
        let written = record_plan_snapshot(
            dir.path(),
            "demo",
            "abc123",
            "v1",
            Some("2025-01-02 03:04:05"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            written.file_name().unwrap().to_string_lossy(),
            "20250102T030405Z-abc123.toml"
        );
    }

    #[tokio::test]
    async fn export_moves_legacy_rows_to_files() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE plans (id INTEGER PRIMARY KEY, name TEXT, plan_checksum TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE plan_snapshots (checksum TEXT PRIMARY KEY, source TEXT NOT NULL, recorded_at DATETIME)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO plans (id, name, plan_checksum) VALUES (1, 'demo', 'aaa')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO plan_snapshots VALUES ('aaa', 'name = \"demo\"', '2025-01-02 03:04:05'), ('zzz', 'orphan source', '2025-03-04 05:06:07')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let exported = export_legacy_plan_snapshots(&pool, dir.path())
            .await
            .unwrap();
        assert_eq!(exported, 2);

        // Named rows land under their plan; the orphan lands in _detached.
        assert_eq!(
            plan_snapshot_source(dir.path(), "demo", "aaa").as_deref(),
            Some("name = \"demo\"")
        );
        let detached = dir.path().join("_detached");
        let names: Vec<String> = std::fs::read_dir(&detached)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["20250304T050607Z-zzz.toml"]);

        // Idempotent: a second export finds the same rows but writes nothing new.
        let again = export_legacy_plan_snapshots(&pool, dir.path())
            .await
            .unwrap();
        assert_eq!(again, 2);
        assert_eq!(std::fs::read_dir(&detached).unwrap().count(), 1);

        // A database that never had the table exports nothing.
        let fresh = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        assert_eq!(
            export_legacy_plan_snapshots(&fresh, dir.path())
                .await
                .unwrap(),
            0
        );
    }
}
