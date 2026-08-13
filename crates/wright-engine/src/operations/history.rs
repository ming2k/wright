use crate::error::Result;
use crate::identify::{Identifier, ResolvedTarget};
use wright_state::database::{HistoryRecord, InstalledDb};

pub async fn execute_history(db: &InstalledDb, target: Option<&str>, json: bool) -> Result<()> {
    let records = gather_history(db, target).await?;

    if json {
        let out: Vec<serde_json::Value> = records
            .iter()
            .map(|r| {
                serde_json::json!({
                    "timestamp": r.timestamp.as_deref(),
                    "session_id": r.session_id.as_str(),
                    "command": r.command.as_str(),
                    "part": r.part_name.as_str(),
                    "action": r.action.to_string(),
                    "old_version": r.old_version.as_deref(),
                    "new_version": r.new_version.as_deref(),
                    "status": r.status.to_string(),
                })
            })
            .collect();
        return super::print_json(&out);
    }

    if records.is_empty() {
        crate::outln!("no history records found");
    } else {
        for r in &records {
            let version = match (&r.old_version, &r.new_version) {
                (None, Some(v)) => v.clone(),
                (Some(v), None) => v.clone(),
                (Some(old), Some(new)) => format!("{} -> {}", old, new),
                (None, None) => String::new(),
            };
            let status = if r.status != wright_state::database::HistoryStatus::Completed {
                format!(" ({})", r.status)
            } else {
                String::new()
            };
            crate::outln!(
                "{}  {:<9} {} {}{}",
                r.timestamp.as_deref().unwrap_or_default(),
                r.action,
                r.part_name,
                version,
                status
            );
        }
    }
    Ok(())
}

/// Collect history rows for an optional target resolved through the
/// universal plan/output identifier: plan-level targets merge the history
/// of every deployed output of the plan, output-level targets show a
/// single output.
async fn gather_history(db: &InstalledDb, target: Option<&str>) -> Result<Vec<HistoryRecord>> {
    let Some(target) = target else {
        return Ok(db.get_history(None).await?);
    };
    let ident = Identifier::parse(target)?;
    match crate::identify::resolve(db, &ident).await? {
        ResolvedTarget::Output { part } => Ok(db.get_history(Some(&part.name)).await?),
        ResolvedTarget::Plan { parts, .. } => {
            let mut records = Vec::new();
            for part in &parts {
                records.extend(db.get_history(Some(&part.name)).await?);
            }
            // Each per-part query is timestamp-ordered; re-sort the merge so
            // the combined listing keeps the same ordering.
            records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            Ok(records)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WrightError;
    use wright_state::database::{HistoryAction, HistoryStatus, NewPart, NewPlan};

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    async fn add_plan(db: &InstalledDb, name: &str, outputs: &[&str]) {
        let plan_id = db
            .insert_plan(NewPlan {
                name,
                ..Default::default()
            })
            .await
            .unwrap();
        for output in outputs {
            db.insert_part(NewPart {
                name: output,
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        }
    }

    async fn add_history(db: &InstalledDb, part_name: &str) {
        db.record_history(
            "session-1",
            &format!("install {}", part_name),
            part_name,
            HistoryAction::Install,
            None,
            Some("1.0.0"),
            None,
            None,
            HistoryStatus::Completed,
            None,
        )
        .await
        .unwrap();
    }

    fn part_names(records: &[HistoryRecord]) -> Vec<&str> {
        let mut names: Vec<&str> = records.iter().map(|r| r.part_name.as_str()).collect();
        names.sort_unstable();
        names
    }

    #[tokio::test]
    async fn history_plan_target_merges_every_output() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "zlib", &["zlib"]).await;
        for part in ["clang", "lld", "zlib"] {
            add_history(&db, part).await;
        }

        for target in ["llvm", "llvm:*"] {
            let records = gather_history(&db, Some(target)).await.unwrap();
            assert_eq!(
                part_names(&records),
                ["clang", "lld"],
                "plan target merges both outputs, zlib stays out of scope"
            );
        }
    }

    #[tokio::test]
    async fn history_output_target_hits_one_part() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        for part in ["clang", "lld"] {
            add_history(&db, part).await;
        }

        for target in ["clang", "llvm:clang"] {
            let records = gather_history(&db, Some(target)).await.unwrap();
            assert_eq!(part_names(&records), ["clang"]);
        }
    }

    #[tokio::test]
    async fn history_rejects_ambiguous_bare_name() {
        let db = test_db().await;
        add_plan(&db, "gcc", &["gcc", "libstdc++"]).await;

        let err = gather_history(&db, Some("gcc")).await.unwrap_err();
        assert!(
            matches!(err, WrightError::AmbiguousTarget(_)),
            "expected ambiguous target, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn history_without_target_returns_everything() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "zlib", &["zlib"]).await;
        for part in ["clang", "lld", "zlib"] {
            add_history(&db, part).await;
        }

        let records = gather_history(&db, None).await.unwrap();
        assert_eq!(part_names(&records), ["clang", "lld", "zlib"]);
    }

    #[tokio::test]
    async fn history_renders_all_target_forms() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_history(&db, "clang").await;

        for target in [
            None,
            Some("llvm"),
            Some("llvm:*"),
            Some("clang"),
            Some("llvm:clang"),
        ] {
            execute_history(&db, target, false).await.unwrap();
            execute_history(&db, target, true).await.unwrap();
        }
    }
}
