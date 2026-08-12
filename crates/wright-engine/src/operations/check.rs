use std::path::Path;
use std::time::Instant;

use crate::error::{Result, WrightError};
use crate::identify::{Identifier, ResolvedTarget};
use wright_state::database::InstalledDb;

/// Implementation of `wright check`.
///
/// Exit semantics: returns `WrightError::DependencyError` when any
/// problem is found, so the CLI dispatch layer maps to a non-zero exit.
#[allow(clippy::too_many_arguments)]
pub async fn execute_check(
    db: &InstalledDb,
    root_dir: &Path,
    target: Option<&str>,
    deep: bool,
    integrity_only: bool,
    check_files: bool,
    json: bool,
) -> Result<()> {
    let t0 = Instant::now();

    // The optional target goes through the universal plan/output
    // identifier: plan-level targets check every deployed output of the
    // plan, output-level targets check a single part.
    let (filter, scope) = match target {
        Some(target) => {
            let ident = Identifier::parse(target)?;
            match crate::identify::resolve(db, &ident).await? {
                ResolvedTarget::Output { part } => {
                    let scope = format!("part '{}'", part.name);
                    (Some(vec![part.name.clone()]), scope)
                }
                ResolvedTarget::Plan { plan, parts } => {
                    let names = parts.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
                    (Some(names), plan.name)
                }
            }
        }
        None => (None, "registry".to_string()),
    };

    let outcome = super::health::run_standard_checks(
        db,
        root_dir,
        filter.as_deref(),
        deep,
        integrity_only,
        check_files,
    )
    .await?;

    if json {
        let report = serde_json::json!({
            "scope": scope,
            "mode": json_mode_label(deep, check_files, integrity_only),
            "issue_count": outcome.total_issues,
            "issues": outcome.issues,
        });
        super::print_json(&report)?;
    }

    if outcome.total_issues == 0 {
        let mode = check_mode_label(deep, check_files, integrity_only);
        crate::cli_action!(
            "Finished",
            "check {} in {}: {} clean",
            mode,
            crate::foundry::logging::format_duration(t0.elapsed().as_secs_f64()),
            scope,
        );
        return Ok(());
    }

    let flag = error_flag_label(deep, check_files);
    Err(WrightError::DependencyError(format!(
        "check{} found {} issue(s)",
        flag, outcome.total_issues,
    )))
}

fn check_mode_label(deep: bool, check_files: bool, integrity_only: bool) -> &'static str {
    if integrity_only {
        "(integrity)"
    } else if deep {
        "(integrity + deps + ELF)"
    } else if check_files {
        "(integrity + files + deps)"
    } else {
        "(integrity + deps)"
    }
}

fn json_mode_label(deep: bool, check_files: bool, integrity_only: bool) -> &'static str {
    if integrity_only {
        "integrity"
    } else if deep {
        "integrity+deps+elf"
    } else if check_files {
        "integrity+files+deps"
    } else {
        "integrity+deps"
    }
}

fn error_flag_label(deep: bool, check_files: bool) -> &'static str {
    if deep {
        " --deep"
    } else if check_files {
        " --files"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_state::database::{FileEntry, FileType, NewPart, NewPlan};

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    /// Insert a plan with deployed outputs, each owning one file so a
    /// `--files` check against an empty root reports one issue per output.
    async fn add_plan(db: &InstalledDb, name: &str, outputs: &[&str]) {
        let plan_id = db
            .insert_plan(NewPlan {
                name,
                ..Default::default()
            })
            .await
            .unwrap();
        for output in outputs {
            let part_id = db
                .insert_part(NewPart {
                    name: output,
                    plan_id,
                    ..Default::default()
                })
                .await
                .unwrap();
            db.insert_files(
                part_id,
                &[FileEntry {
                    path: format!("/usr/bin/{}", output),
                    file_hash: None,
                    file_type: FileType::File,
                    file_mode: None,
                    file_size: None,
                    is_config: false,
                }],
            )
            .await
            .unwrap();
        }
    }

    /// Run a `--files` check against a root where every deployed file is
    /// missing; the failure message carries the issue count.
    async fn check_files_failure(db: &InstalledDb, target: Option<&str>) -> String {
        let err = execute_check(
            db,
            Path::new("/nonexistent-root"),
            target,
            false,
            false,
            true,
            false,
        )
        .await
        .unwrap_err();
        let WrightError::DependencyError(msg) = err else {
            panic!("expected dependency error, got: {}", err);
        };
        msg
    }

    #[tokio::test]
    async fn check_plan_target_checks_every_output() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "zlib", &["zlib"]).await;

        // Both of llvm's outputs are checked; zlib stays out of scope.
        for target in ["llvm", "llvm:*"] {
            let msg = check_files_failure(&db, Some(target)).await;
            assert!(
                msg.contains("2 issue(s)"),
                "plan target covers both outputs: {}",
                msg
            );
        }

        // No target keeps the registry-level scope: all three parts.
        let msg = check_files_failure(&db, None).await;
        assert!(
            msg.contains("3 issue(s)"),
            "registry scope covers every part: {}",
            msg
        );
    }

    #[tokio::test]
    async fn check_output_target_checks_one_part() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;

        for target in ["clang", "llvm:clang"] {
            let msg = check_files_failure(&db, Some(target)).await;
            assert!(
                msg.contains("1 issue(s)"),
                "output target covers one part: {}",
                msg
            );
        }
    }

    #[tokio::test]
    async fn check_rejects_ambiguous_bare_name() {
        let db = test_db().await;
        add_plan(&db, "gcc", &["gcc", "libstdc++"]).await;

        let err = execute_check(&db, Path::new("/"), Some("gcc"), false, false, true, false)
            .await
            .unwrap_err();
        assert!(
            matches!(err, WrightError::AmbiguousTarget(_)),
            "expected ambiguous target, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn check_plan_without_outputs_is_not_found() {
        let db = test_db().await;
        add_plan(&db, "empty", &[]).await;

        let err = execute_check(
            &db,
            Path::new("/"),
            Some("empty"),
            false,
            false,
            false,
            false,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, WrightError::PartNotFound(_)),
            "expected part not found, got: {}",
            err
        );
    }
}
