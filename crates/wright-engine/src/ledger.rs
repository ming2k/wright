//! Machine-local audit ledger paths and build-cost records.
//!
//! The ledger (`general.ledger_dir`, default `/var/lib/wright/ledger`) holds
//! per-plan audit data as plain files:
//!
//! ```text
//! <ledger>/<plan>/builds.jsonl                     one record per build
//! <ledger>/<plan>/snapshots/<ts>-<checksum>.toml   plan-source history
//! ```
//!
//! Everything here is *advisory*: a ledger write that fails (read-only
//! prefix, non-root build against the system ledger) warns and is dropped —
//! it must never fail a build, seal, or deploy. Reads degrade to `None`.

use std::path::{Path, PathBuf};

use crate::config::GlobalConfig;

/// Resolve the ledger directory for a database. A redirected database
/// (`--root`, `--db`) keeps its ledger beside itself — the same derivation
/// rule the journal and lock dirs already use — so a target root's ledger
/// never mixes with the host's. The configured `ledger_dir` governs only
/// the configured (default) database.
pub fn dir(config: &GlobalConfig, db_path: Option<&Path>) -> PathBuf {
    match db_path {
        Some(path) if path != config.general.db_path => path
            .parent()
            .map(|parent| parent.join("ledger"))
            .unwrap_or_else(|| config.general.ledger_dir.clone()),
        _ => config.general.ledger_dir.clone(),
    }
}

mod builds;

pub use builds::{BuildRecord, StageTiming, append_build_record, dir_stats};
// The snapshot store lives in wright-state (it also serves the legacy-table
// export at open time); re-export it here so `wright::ledger` is the single
// public entry point for everything ledger-related.
pub use wright_state::ledger::{plan_snapshot_source, record_plan_snapshot};
