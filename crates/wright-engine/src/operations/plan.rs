use std::path::Path;

use crate::error::{Result, WrightError};
use wright_state::database::InstalledDb;

/// Print the plan-source snapshot recorded when the plan's parts were
/// sealed (ADR-0033, ledger layout ADR-0041). Text mode emits the exact
/// plan.toml bytes so the output can round-trip onto disk
/// (`wright plan zlib > plan.toml`).
pub async fn execute_plan(
    db: &InstalledDb,
    ledger_dir: &Path,
    name: &str,
    json: bool,
) -> Result<()> {
    let (checksum, source) = plan_snapshot(db, ledger_dir, name).await?;

    if json {
        #[derive(serde::Serialize)]
        struct PlanSnapshot<'a> {
            plan: &'a str,
            checksum: &'a str,
            source: &'a str,
        }
        super::print_json(&PlanSnapshot {
            plan: name,
            checksum: &checksum,
            source: &source,
        })
    } else {
        crate::out!("{}", source);
        Ok(())
    }
}

/// Look up the recorded plan-source snapshot for an installed plan.
/// Returns `(plan_checksum, plan_source)`.
async fn plan_snapshot(
    db: &InstalledDb,
    ledger_dir: &Path,
    name: &str,
) -> Result<(String, String)> {
    let plan = db
        .get_plan(name)
        .await?
        .ok_or_else(|| WrightError::PartNotFound(format!("plan '{}'", name)))?;

    let checksum = plan.plan_checksum.clone().ok_or_else(|| {
        WrightError::ValidationError(format!(
            "plan '{}' has no recorded provenance (parts sealed before ADR-0023); \
             rebuild and re-deploy to record one",
            name
        ))
    })?;

    let source = wright_state::ledger::plan_snapshot_source(ledger_dir, name, &checksum)
        .ok_or_else(|| {
            WrightError::ValidationError(format!(
                "no plan-source snapshot recorded for '{}' (parts sealed before ADR-0033); \
                 rebuild and re-deploy to record one",
                name
            ))
        })?;

    Ok((checksum, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::{NewPlan, NewPlanProvenance, RegisterPlan};

    async fn register_plan(
        db: &InstalledDb,
        ledger_dir: &Path,
        name: &str,
        checksum: Option<&str>,
        snapshot: Option<&str>,
    ) {
        db.ensure_plan_registered(RegisterPlan {
            plan: NewPlan {
                name,
                version: "1.0.0",
                release: 1,
                epoch: 0,
                arch: "x86_64",
            },
            provenance: checksum.map(|sum| NewPlanProvenance {
                plan_checksum: Some(sum),
                source_checksums: &[],
                wright_version: "test",
                isolation: "none",
            }),
        })
        .await
        .unwrap();
        if let (Some(sum), Some(source)) = (checksum, snapshot) {
            wright_state::ledger::record_plan_snapshot(ledger_dir, name, sum, source, None)
                .unwrap();
        }
    }

    #[tokio::test]
    async fn snapshot_returns_recorded_source() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        let ledger = tempfile::tempdir().unwrap();
        register_plan(
            &db,
            ledger.path(),
            "demo",
            Some("deadbeef"),
            Some("release = 1\n"),
        )
        .await;

        let (checksum, source) = plan_snapshot(&db, ledger.path(), "demo").await.unwrap();
        assert_eq!(checksum, "deadbeef");
        assert_eq!(source, "release = 1\n");
    }

    #[tokio::test]
    async fn unknown_plan_is_not_found() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        let ledger = tempfile::tempdir().unwrap();
        let err = plan_snapshot(&db, ledger.path(), "ghost")
            .await
            .unwrap_err();
        assert!(matches!(err, WrightError::PartNotFound(_)), "got: {}", err);
    }

    #[tokio::test]
    async fn missing_checksum_or_snapshot_is_a_clear_error() {
        let db = InstalledDb::open_in_memory().await.unwrap();
        let ledger = tempfile::tempdir().unwrap();
        register_plan(&db, ledger.path(), "ancient", None, None).await;
        let err = plan_snapshot(&db, ledger.path(), "ancient")
            .await
            .unwrap_err();
        assert!(
            matches!(err, WrightError::ValidationError(_)),
            "got: {}",
            err
        );

        register_plan(&db, ledger.path(), "pre-snapshot", Some("deadbeef"), None).await;
        let err = plan_snapshot(&db, ledger.path(), "pre-snapshot")
            .await
            .unwrap_err();
        assert!(
            matches!(err, WrightError::ValidationError(_)),
            "got: {}",
            err
        );
    }
}
