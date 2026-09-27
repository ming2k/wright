//! File-backed immutable audit ledger and plan snapshots (ADR-0041, ADR-0048).
//!
//! Layout:
//! ```text
//! <ledger_dir>/<plan>/snapshots/<yyyymmddThhmmssZ>-<sha256>.toml
//! <ledger_dir>/_detached/<yyyymmddThhmmssZ>-<sha256>.toml
//! <ledger_dir>/<plan>/builds.jsonl
//! ```

use std::path::{Path, PathBuf};

pub mod error;
pub use error::{LedgerError, Result};

pub const DETACHED_DIR: &str = "_detached";

/// Record a plan-source snapshot unless an identical one (same checksum)
/// already exists for the plan.
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
    std::fs::create_dir_all(&dir).map_err(|e| LedgerError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let file_name = format!("{}-{}.toml", snapshot_timestamp(recorded_at), checksum);
    let path = dir.join(&file_name);
    let tmp = dir.join(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, source).map_err(|e| LedgerError::Io {
        path: tmp.clone(),
        source: e,
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| LedgerError::Io {
        path: path.clone(),
        source: e,
    })?;
    Ok(Some(path))
}

/// Fetch a recorded snapshot by plan and checksum.
pub fn plan_snapshot_source(ledger_dir: &Path, plan: &str, checksum: &str) -> Option<String> {
    let dir = snapshot_dir(ledger_dir, plan);
    let path = find_snapshot(&dir, checksum).ok()??;
    std::fs::read_to_string(path).ok()
}

pub fn snapshot_dir(ledger_dir: &Path, plan: &str) -> PathBuf {
    ledger_dir.join(plan).join("snapshots")
}

pub fn find_snapshot(dir: &Path, checksum: &str) -> Result<Option<PathBuf>> {
    let suffix = format!("-{}.toml", checksum);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(LedgerError::Io {
                path: dir.to_path_buf(),
                source: e,
            });
        }
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().ends_with(&suffix) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

pub fn snapshot_timestamp(recorded_at: Option<&str>) -> String {
    recorded_at
        .and_then(|raw| {
            chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| dt.format("%Y%m%dT%H%M%SZ").to_string())
        })
        .unwrap_or_else(|| chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_deduplicated_by_checksum() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = tmp.path();

        let first = record_plan_snapshot(ledger, "demo", "abc1234", "plan source 1", None)
            .unwrap()
            .expect("first snapshot written");
        assert!(first.exists());

        // Same checksum: skipped
        let second = record_plan_snapshot(ledger, "demo", "abc1234", "plan source 1 modified", None)
            .unwrap();
        assert!(second.is_none());

        // Different checksum: written
        let third = record_plan_snapshot(ledger, "demo", "def5678", "plan source 2", None)
            .unwrap()
            .expect("second checksum written");
        assert!(third.exists());
        assert_ne!(first, third);

        // Read back
        assert_eq!(
            plan_snapshot_source(ledger, "demo", "abc1234").as_deref(),
            Some("plan source 1")
        );
        assert_eq!(
            plan_snapshot_source(ledger, "demo", "def5678").as_deref(),
            Some("plan source 2")
        );
        assert_eq!(plan_snapshot_source(ledger, "demo", "unknown"), None);
    }
}
