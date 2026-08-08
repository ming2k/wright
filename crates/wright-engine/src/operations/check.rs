use std::path::Path;
use std::time::Instant;

use crate::error::{Result, WrightError};
use wright_state::database::InstalledDb;

/// Implementation of `wright check`.
///
/// Exit semantics: returns `WrightError::DependencyError` when any
/// problem is found, so the CLI dispatch layer maps to a non-zero exit.
#[allow(clippy::too_many_arguments)]
pub async fn execute_check(
    db: &InstalledDb,
    root_dir: &Path,
    only_part: Option<&str>,
    deep: bool,
    integrity_only: bool,
    check_files: bool,
    json: bool,
) -> Result<()> {
    let t0 = Instant::now();
    let outcome = super::health::run_standard_checks(
        db,
        root_dir,
        only_part,
        deep,
        integrity_only,
        check_files,
    )
    .await?;

    if json {
        let report = serde_json::json!({
            "scope": scope_label(only_part),
            "mode": json_mode_label(deep, check_files, integrity_only),
            "issue_count": outcome.total_issues,
            "issues": outcome.issues,
        });
        super::print_json(&report)?;
    }

    if outcome.total_issues == 0 {
        let scope = scope_label(only_part);
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

fn scope_label(only_part: Option<&str>) -> String {
    match only_part {
        Some(p) => format!("part '{}'", p),
        None => "registry".to_string(),
    }
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
