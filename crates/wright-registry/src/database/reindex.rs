//! Registry rebuild from the inventory (ADR-0043).
//!
//! `wright.db` is a derived index over facts that already exist: on the live
//! root, and in `.PARTINFO`/`.FILELIST` inside every archive in `parts_dir`.
//! When a destructive update or a failed migration damages the database, the
//! ownership facts are still on disk in the inventory, and this module puts
//! them back.
//!
//! What is *not* rederivable is called out rather than faked: the `history`
//! table is left untouched (it is an audit log, not a derivation of the
//! inventory), and a part's original `installed_at` timestamp and `origin` are
//! not recoverable from an archive. Callers may supply a prior origin so a
//! rebuild preserves what the damaged database still knew.

use super::{FileType, InstalledDb, Origin};
use crate::error::{Result, WrightError};
use rusqlite::{Transaction, params};
use std::collections::HashMap;

/// One file to (re)register under a part.
#[derive(Debug, Clone)]
pub struct RebuiltFile {
    pub path: String,
    pub file_type: FileType,
    /// Recorded size, or `None` when the path is absent from the live root.
    pub file_size: Option<i64>,
}

/// One part to (re)register, derived from an archive in the inventory.
#[derive(Debug, Clone)]
pub struct RebuiltPart {
    pub name: String,
    /// SHA-256 of the archive file, matching what install records.
    pub part_hash: Option<String>,
    pub origin: Origin,
    pub files: Vec<RebuiltFile>,
    pub dependencies: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
}

/// One plan and its outputs to (re)register.
#[derive(Debug, Clone)]
pub struct RebuiltPlan {
    pub name: String,
    pub version: String,
    pub release: u32,
    pub epoch: u32,
    pub arch: String,
    pub plan_checksum: Option<String>,
    pub source_checksums: Vec<String>,
    pub wright_version: Option<String>,
    pub isolation: Option<String>,
    pub parts: Vec<RebuiltPart>,
}

/// Counts of what a rebuild wrote, for the command's summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildSummary {
    pub plans: usize,
    pub parts: usize,
    pub files: usize,
    pub dependencies: usize,
}

impl InstalledDb {
    /// Replace the registry wholesale with `plans`, atomically.
    ///
    /// `preserve_origins` carries part-name → origin from a best-effort read of
    /// the pre-rebuild database, so an install that was known to be a root
    /// `forge` or a pulled-in `dependency` keeps that classification instead of
    /// collapsing to `manual`.
    ///
    /// The whole rebuild runs in one immediate transaction: a crash mid-rebuild
    /// leaves the previous registry intact rather than a half-populated one.
    /// The `history` table is never touched.
    pub async fn rebuild_registry(
        &self,
        plans: Vec<RebuiltPlan>,
        preserve_origins: HashMap<String, Origin>,
    ) -> Result<RebuildSummary> {
        self.write(move |conn| {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| WrightError::context("failed to begin registry rebuild", e))?;

            clear_registry(&tx)?;

            let mut summary = RebuildSummary::default();
            for plan in &plans {
                let plan_id = insert_plan(&tx, plan)?;
                summary.plans += 1;

                for part in &plan.parts {
                    let origin = preserve_origins
                        .get(&part.name)
                        .copied()
                        .unwrap_or(part.origin);
                    let part_id = insert_part(&tx, plan_id, part, origin)?;
                    summary.parts += 1;

                    for file in &part.files {
                        tx.execute(
                            "INSERT INTO files (part_id, path, file_hash, file_type, file_mode, file_size, is_config)
                             VALUES (?1, ?2, NULL, ?3, NULL, ?4, 0)",
                            params![part_id, file.path, file.file_type, file.file_size],
                        )
                        .map_err(|e| WrightError::context("failed to insert rebuilt file", e))?;
                        summary.files += 1;
                    }

                    for dep in &part.dependencies {
                        tx.execute(
                            "INSERT INTO dependencies (part_id, depends_on, version_constraint)
                             VALUES (?1, ?2, NULL)",
                            params![part_id, dep],
                        )
                        .map_err(|e| WrightError::context("failed to insert rebuilt dependency", e))?;
                        summary.dependencies += 1;
                    }

                    for conflict in &part.conflicts {
                        tx.execute(
                            "INSERT INTO conflicts (part_id, name) VALUES (?1, ?2)",
                            params![part_id, conflict],
                        )
                        .map_err(|e| WrightError::context("failed to insert rebuilt conflict", e))?;
                    }

                    for replace in &part.replaces {
                        tx.execute(
                            "INSERT INTO replaces (part_id, name) VALUES (?1, ?2)",
                            params![part_id, replace],
                        )
                        .map_err(|e| WrightError::context("failed to insert rebuilt replaces", e))?;
                    }
                }
            }

            tx.commit()
                .map_err(|e| WrightError::context("failed to commit registry rebuild", e))?;
            Ok(summary)
        })
        .await
    }
}

/// Empty every derivation table. Order is child-first so the deletes are valid
/// with foreign keys on; `ON DELETE CASCADE` would also cover it, but explicit
/// order keeps the intent readable.
fn clear_registry(tx: &Transaction<'_>) -> Result<()> {
    for table in [
        "shadowed_files",
        "conflicts",
        "replaces",
        "dependencies",
        "files",
        "parts",
        "plans",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])
            .map_err(|e| WrightError::context(format!("failed to clear {table}"), e))?;
    }
    Ok(())
}

fn insert_plan(tx: &Transaction<'_>, plan: &RebuiltPlan) -> Result<i64> {
    let source_checksums = serde_json::to_string(&plan.source_checksums)
        .map_err(|e| WrightError::context("serialize rebuilt source_checksums", e))?;
    tx.execute(
        "INSERT INTO plans (name, version, release, epoch, arch, plan_checksum, source_checksums, wright_version, isolation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            plan.name,
            plan.version,
            plan.release as i64,
            plan.epoch as i64,
            plan.arch,
            plan.plan_checksum,
            source_checksums,
            plan.wright_version,
            plan.isolation,
        ],
    )
    .map_err(|e| WrightError::context("failed to insert rebuilt plan", e))?;
    Ok(tx.last_insert_rowid())
}

fn insert_part(
    tx: &Transaction<'_>,
    plan_id: i64,
    part: &RebuiltPart,
    origin: Origin,
) -> Result<i64> {
    // `installed_at` is deliberately NULL: the archive does not record when the
    // part was deployed, and stamping "now" would present a fabricated fact as
    // an audit record.
    tx.execute(
        "INSERT INTO parts (name, plan_id, installed_at, part_hash, deploy_scripts, origin)
         VALUES (?1, ?2, NULL, ?3, NULL, ?4)",
        params![part.name, plan_id, part.part_hash, origin],
    )
    .map_err(|e| WrightError::context("failed to insert rebuilt part", e))?;
    Ok(tx.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(name: &str, file: &str) -> RebuiltPart {
        RebuiltPart {
            name: name.to_string(),
            part_hash: Some("deadbeef".to_string()),
            origin: Origin::Manual,
            files: vec![RebuiltFile {
                path: file.to_string(),
                file_type: FileType::File,
                file_size: Some(10),
            }],
            dependencies: vec!["libc".to_string()],
            conflicts: vec!["old-tool".to_string()],
            replaces: vec!["legacy".to_string()],
        }
    }

    fn plan(name: &str, parts: Vec<RebuiltPart>) -> RebuiltPlan {
        RebuiltPlan {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            release: 1,
            epoch: 0,
            arch: "x86_64".to_string(),
            plan_checksum: Some("cafe".to_string()),
            source_checksums: vec!["http https://x sha256=a".to_string()],
            wright_version: Some("9.9.9".to_string()),
            isolation: Some("strict".to_string()),
            parts,
        }
    }

    #[tokio::test]
    async fn rebuild_replaces_registry_and_keeps_history() {
        let db = InstalledDb::open_in_memory().await.unwrap();

        // Seed a stale registry plus one history row that must survive.
        db.insert_plan(super::super::NewPlan {
            name: "stale",
            version: "0.1.0",
            release: 1,
            epoch: 0,
            arch: "x86_64",
        })
        .await
        .unwrap();
        db.insert_part(super::super::NewPart {
            name: "stale",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        db.record_history(
            "s1",
            "install",
            "stale",
            super::super::HistoryAction::Install,
            None,
            Some("0.1.0"),
            None,
            None,
            super::super::HistoryStatus::Completed,
            None,
        )
        .await
        .unwrap();

        let summary = db
            .rebuild_registry(
                vec![plan("demo", vec![part("demo", "/usr/bin/demo")])],
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            summary,
            RebuildSummary {
                plans: 1,
                parts: 1,
                files: 1,
                dependencies: 1
            }
        );

        // The old plan is gone; the new one is present with its files/deps.
        assert!(db.get_plan("stale").await.unwrap().is_none());
        let rebuilt = db.get_plan("demo").await.unwrap().unwrap();
        assert_eq!(rebuilt.plan_checksum.as_deref(), Some("cafe"));
        let parts = db.list_parts().await.unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "demo");
        assert!(
            parts[0].installed_at.is_none(),
            "installed_at must stay unknown"
        );
        let files = db.get_files(parts[0].id).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/bin/demo");

        // History is an audit log, not a derivation — it survives untouched.
        let history = db.get_history(None).await.unwrap();
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn preserve_origins_keeps_dependency_classification() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        let mut origins = HashMap::new();
        origins.insert("libfoo".to_string(), Origin::Dependency);

        db.rebuild_registry(
            vec![plan("demo", vec![part("libfoo", "/usr/lib/libfoo.so")])],
            origins,
        )
        .await
        .unwrap();

        let parts = db.list_parts().await.unwrap();
        assert_eq!(parts[0].origin, Origin::Dependency);
    }
}
