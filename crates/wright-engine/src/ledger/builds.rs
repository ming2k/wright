//! Per-plan build-cost records: `<ledger>/<plan>/builds.jsonl`.
//!
//! One JSON object per line, appended at the end of every forge attempt —
//! success or failure. The file is append-only; nothing rotates or deletes
//! it, so the history doubles as the answer to "what will the next upgrade
//! of this plan cost me?" (durations, download/staging sizes, failure rate)
//! using plain `tail` / `jq`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use wright_part::platform::HostInfo;

use crate::error::{Result, WrightError};

/// One build attempt. Field set is stable audit data — extend only by
/// adding fields, never by renaming, so old lines stay parseable.
#[derive(Debug, Serialize)]
pub struct BuildRecord {
    /// RFC 3339 UTC timestamp of the attempt's end.
    pub ts: String,
    /// Always `"build"` — the file may gain sibling record types later.
    pub kind: &'static str,
    pub plan: String,
    pub version: String,
    pub release: u32,
    /// SHA-256 of the plan.toml that drove the build, when the manifest was
    /// loaded from a file.
    pub plan_checksum: Option<String>,
    /// Build host summary (CPU flags stripped — see `.BUILDINFO` for those).
    pub host: HostInfo,
    /// Version of the `wright` binary that ran the build; toolchain changes
    /// explain cost drift that hardware does not.
    pub wright_version: String,
    pub success: bool,
    /// Flattened error chain on failure.
    pub error: Option<String>,
    /// `false` for partial runs (`--stage`, `--fetch-only`, `--until-stage`):
    /// their durations cover a subset of the pipeline and would skew
    /// full-build cost estimates.
    pub full: bool,
    /// Wall-clock seconds for the whole attempt.
    pub duration_secs: f64,
    /// Per-step seconds in execution order (`charge`, each forge stage that
    /// ran, `slice`). Cached/skipped stages are absent.
    pub stages: Vec<StageTiming>,
    /// Total size of the plan's cached source archives (download cost).
    pub source_bytes: Option<u64>,
    /// Total size of the staging tree (installed-footprint cost).
    pub staging_bytes: Option<u64>,
    /// File count of the staging tree.
    pub staging_files: Option<u64>,
}

/// One timed step inside a build attempt.
#[derive(Debug, Serialize)]
pub struct StageTiming {
    pub name: String,
    pub secs: f64,
    pub ok: bool,
}

/// Append one record to `<ledger_dir>/<plan>/builds.jsonl`. A single
/// `write_all` under O_APPEND keeps concurrent builders' lines intact.
pub fn append_build_record(ledger_dir: &Path, record: &BuildRecord) -> Result<PathBuf> {
    let dir = ledger_dir.join(&record.plan);
    std::fs::create_dir_all(&dir).map_err(|e| {
        WrightError::context(format!("failed to create ledger dir {}", dir.display()), e)
    })?;
    let path = dir.join("builds.jsonl");
    let mut line = serde_json::to_string(record)
        .map_err(|e| WrightError::context("failed to serialize build record", e))?;
    line.push('\n');
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| {
            WrightError::context(format!("failed to open ledger {}", path.display()), e)
        })?;
    file.write_all(line.as_bytes())
        .map_err(|e| WrightError::context(format!("failed to append to {}", path.display()), e))?;
    Ok(path)
}

/// Total byte size and file count of a directory tree (0, 0 when missing).
pub fn dir_stats(dir: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut files = 0;
    for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
        if entry.file_type().is_file() {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(plan: &str) -> BuildRecord {
        BuildRecord {
            ts: "2026-08-14T01:40:23Z".to_string(),
            kind: "build",
            plan: plan.to_string(),
            version: "1.0.0".to_string(),
            release: 1,
            plan_checksum: Some("abc123".to_string()),
            host: HostInfo::probe(false),
            wright_version: "test".to_string(),
            success: true,
            error: None,
            full: true,
            duration_secs: 12.5,
            stages: vec![StageTiming {
                name: "compile".to_string(),
                secs: 12.0,
                ok: true,
            }],
            source_bytes: Some(1024),
            staging_bytes: Some(2048),
            staging_files: Some(3),
        }
    }

    #[test]
    fn appends_one_json_line_per_record() {
        let dir = tempfile::tempdir().unwrap();
        append_build_record(dir.path(), &record("demo")).unwrap();
        append_build_record(dir.path(), &record("demo")).unwrap();

        let content = std::fs::read_to_string(dir.path().join("demo/builds.jsonl")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["kind"], "build");
        assert_eq!(parsed["plan"], "demo");
        assert_eq!(parsed["stages"][0]["name"], "compile");
        // Flags are stripped from ledger records (they live in .BUILDINFO).
        assert!(parsed["host"].get("cpu_flags").is_none());
    }

    #[test]
    fn dir_stats_counts_files_and_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("usr/bin")).unwrap();
        std::fs::write(dir.path().join("usr/bin/a"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.path().join("usr/b"), vec![0u8; 5]).unwrap();
        assert_eq!(dir_stats(dir.path()), (15, 2));
        assert_eq!(dir_stats(&dir.path().join("missing")), (0, 0));
    }
}
