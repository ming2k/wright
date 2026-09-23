//! Installed-state schema migrations.

use crate::error::{Result, WrightError};
use rusqlite::{Connection, TransactionBehavior};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
}

pub const CURRENT_DB_VERSION: u32 = 20;

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "001_initial_schema.sql",
        sql: include_str!("../../migrations/001_initial_schema.sql"),
    },
    Migration {
        version: 2,
        name: "002_execution_sessions.sql",
        sql: include_str!("../../migrations/002_execution_sessions.sql"),
    },
    Migration {
        version: 3,
        name: "003_add_plan_name.sql",
        sql: include_str!("../../migrations/003_add_plan_name.sql"),
    },
    Migration {
        version: 4,
        name: "004_separate_plan_part_deps.sql",
        sql: include_str!("../../migrations/004_separate_plan_part_deps.sql"),
    },
    Migration {
        version: 5,
        name: "005_hard_refactor_plan_output.sql",
        sql: include_str!("../../migrations/005_hard_refactor_plan_output.sql"),
    },
    Migration {
        version: 6,
        name: "006_extract_plan_deps.sql",
        sql: include_str!("../../migrations/006_extract_plan_deps.sql"),
    },
    Migration {
        version: 7,
        name: "007_assumed_to_origin_external.sql",
        sql: include_str!("../../migrations/007_assumed_to_origin_external.sql"),
    },
    Migration {
        version: 8,
        name: "008_cleanup.sql",
        sql: include_str!("../../migrations/008_cleanup.sql"),
    },
    Migration {
        version: 9,
        name: "009_workflow.sql",
        sql: include_str!("../../migrations/009_workflow.sql"),
    },
    Migration {
        version: 10,
        name: "010_drop_legacy_sessions.sql",
        sql: include_str!("../../migrations/010_drop_legacy_sessions.sql"),
    },
    Migration {
        version: 11,
        name: "011_drop_build_sessions.sql",
        sql: include_str!("../../migrations/011_drop_build_sessions.sql"),
    },
    Migration {
        version: 12,
        name: "012_rework_workflow_state.sql",
        sql: include_str!("../../migrations/012_rework_workflow_state.sql"),
    },
    Migration {
        version: 13,
        name: "013_advisory_runtime_deps.sql",
        sql: include_str!("../../migrations/013_advisory_runtime_deps.sql"),
    },
    Migration {
        version: 14,
        name: "014_drop_doc_fields.sql",
        sql: include_str!("../../migrations/014_drop_doc_fields.sql"),
    },
    Migration {
        version: 15,
        name: "015_delivery_transactions.sql",
        sql: include_str!("../../migrations/015_delivery_transactions.sql"),
    },
    Migration {
        version: 16,
        name: "016_history_redesign.sql",
        sql: include_str!("../../migrations/016_history_redesign.sql"),
    },
    Migration {
        version: 17,
        name: "017_plan_provenance.sql",
        sql: include_str!("../../migrations/017_plan_provenance.sql"),
    },
    Migration {
        version: 18,
        name: "018_plan_snapshots.sql",
        sql: include_str!("../../migrations/018_plan_snapshots.sql"),
    },
    Migration {
        version: 19,
        name: "019_normalize_dependency_edges.sql",
        sql: include_str!("../../migrations/019_normalize_dependency_edges.sql"),
    },
    Migration {
        version: 20,
        name: "020_file_ledger_plan_snapshots.sql",
        sql: include_str!("../../migrations/020_file_ledger_plan_snapshots.sql"),
    },
];

/// Apply hardware-conscious PRAGMAs calibrated for SSD durability, WAL ring
/// buffering, and zero-churn concurrency (ADR-0042).
pub fn configure_connection(conn: &mut Connection) -> Result<()> {
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| WrightError::context("failed to set busy_timeout", e))?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| WrightError::context("failed to set synchronous", e))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| WrightError::context("failed to enable foreign keys", e))?;
    conn.pragma_update(None, "journal_size_limit", 16777216)
        .map_err(|e| WrightError::context("failed to set journal_size_limit", e))?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000)
        .map_err(|e| WrightError::context("failed to set wal_autocheckpoint", e))?;
    conn.pragma_update(None, "temp_store", "MEMORY")
        .map_err(|e| WrightError::context("failed to set temp_store", e))?;
    Ok(())
}

/// Run pending migrations up to `CURRENT_DB_VERSION`.
///
/// When `db_path` is supplied and there is work to do, a `VACUUM INTO`
/// snapshot of the pre-migration database is written beside it before the
/// first migration runs (ADR-0043). A destructive migration then costs a
/// restore instead of a system. `None` skips snapshotting (in-memory and
/// test databases).
pub fn run_migrations(conn: &mut Connection, db_path: Option<&Path>) -> Result<()> {
    let mut current_version: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| WrightError::context("failed to read user_version", e))?;

    // Check for legacy _sqlx_migrations metadata if user_version is uninitialized
    if current_version == 0 {
        let has_sqlx: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(|e| WrightError::context("failed to inspect legacy migrations table", e))?
            > 0;

        if has_sqlx {
            let max_v: Option<i64> = conn
                .query_row("SELECT MAX(version) FROM _sqlx_migrations", [], |r| {
                    r.get(0)
                })
                .unwrap_or(None);
            if let Some(v) = max_v
                && v > 0
            {
                current_version = v as u32;
                conn.pragma_update(None, "user_version", current_version)
                    .map_err(|e| {
                        WrightError::context("failed to stamp user_version from legacy table", e)
                    })?;
            }
        }
    }

    if current_version > CURRENT_DB_VERSION {
        return Err(WrightError::DatabaseError(format!(
            "database schema v{current_version} is newer than this binary (v{CURRENT_DB_VERSION}); please upgrade wright"
        )));
    }

    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        return Ok(());
    }

    let target_version = pending.last().map(|m| m.version).unwrap_or(current_version);

    // Snapshot the pre-migration database before touching it. A failure to
    // snapshot is fatal: proceeding would apply a destructive migration with
    // no way back, which is the exact scenario this guards against.
    let snapshot = match db_path {
        Some(path) => Some(snapshot_before_migration(
            conn,
            path,
            current_version,
            target_version,
        )?),
        None => None,
    };

    info!(
        "updating database schema ({} changes pending, current v{})",
        pending.len(),
        current_version
    );

    // Disable foreign keys during schema restructuring migrations
    conn.pragma_update(None, "foreign_keys", "OFF")
        .map_err(|e| {
            WrightError::context(
                "failed to temporarily disable foreign keys for migration",
                e,
            )
        })?;

    let outcome = (|| -> Result<()> {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| WrightError::context("failed to begin migration transaction", e))?;

        for m in pending {
            tx.execute_batch(m.sql).map_err(|e| {
                let recovery = match &snapshot {
                    Some(path) => format!(
                        "; restore with `wright doctor --restore {}` or rebuild with `wright doctor --repair`",
                        path.display()
                    ),
                    None => "; rebuild with `wright doctor --repair`".to_string(),
                };
                WrightError::context(
                    format!("failed executing migration {}{}", m.name, recovery),
                    e,
                )
            })?;

            tx.pragma_update(None, "user_version", m.version)
                .map_err(|e| {
                    WrightError::context(format!("failed to update user_version for {}", m.name), e)
                })?;
        }

        tx.commit()
            .map_err(|e| WrightError::context("failed to commit migrations", e))?;
        Ok(())
    })();

    // Always re-enable foreign keys
    let restore = conn
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| WrightError::context("failed to restore foreign keys after migration", e));

    outcome?;
    restore?;

    if let Some(path) = &snapshot {
        info!(
            event = "db.migrated",
            from_version = current_version,
            to_version = target_version,
            snapshot = %path.display(),
            "database schema migrated; pre-migration snapshot retained"
        );
    }

    Ok(())
}

/// How many pre-migration snapshots to keep beside the database.
const SNAPSHOT_RETENTION: usize = 3;

/// Write a `VACUUM INTO` snapshot of the current database beside it, and prune
/// older snapshots.
///
/// `VACUUM INTO` produces a single, consistent, standalone file and is
/// therefore immune to the WAL hazard that makes hand-copying `wright.db`
/// unsafe: under `journal_mode = WAL` the `-wal` sidecar can hold committed
/// pages that have not been checkpointed into the main file, so a plain file
/// copy can miss recent writes or capture a torn state.
fn snapshot_before_migration(
    conn: &Connection,
    db_path: &Path,
    from_version: u32,
    to_version: u32,
) -> Result<PathBuf> {
    let base = db_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wright.db".to_string());
    let snapshot = db_path.with_file_name(format!(
        "{base}.pre-migrate-v{from_version}-to-v{to_version}.bak"
    ));

    // VACUUM INTO refuses an existing target; a rerun after a failed migration
    // must be able to overwrite the previous attempt's snapshot.
    let _ = std::fs::remove_file(&snapshot);

    conn.execute("VACUUM INTO ?1", [snapshot.to_string_lossy().as_ref()])
        .map_err(|e| {
            WrightError::context(
                format!(
                    "failed to snapshot database to {} before migrating v{} -> v{}",
                    snapshot.display(),
                    from_version,
                    to_version
                ),
                e,
            )
        })?;

    prune_snapshots(db_path, SNAPSHOT_RETENTION);
    Ok(snapshot)
}

/// Keep only the `keep` most recent `*.pre-migrate-*.bak` snapshots beside the
/// database, so the recovery mechanism cannot itself grow without bound.
fn prune_snapshots(db_path: &Path, keep: usize) {
    let Some(dir) = db_path.parent() else {
        return;
    };
    let base = db_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prefix = format!("{base}.pre-migrate-");

    let mut snapshots: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) || !name.ends_with(".bak") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        snapshots.push((modified, entry.path()));
    }

    if snapshots.len() <= keep {
        return;
    }
    snapshots.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in snapshots.into_iter().skip(keep) {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!(
                event = "db.snapshot_prune_failed",
                path = %path.display(),
                error = %e,
                "failed to prune an old pre-migration snapshot"
            );
        }
    }
}
