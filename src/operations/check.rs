use std::path::Path;

use owo_colors::OwoColorize;

use crate::database::InstalledDb;
use crate::error::{Result, WrightError};

/// Implementation of `wright check`.
///
/// Exit semantics: returns `WrightError::DependencyError` when any
/// problem is found, so the CLI dispatch layer maps to a non-zero exit.
pub async fn execute_check(
    db: &InstalledDb,
    root_dir: &Path,
    only_part: Option<&str>,
    deep: bool,
    integrity_only: bool,
    check_files: bool,
) -> Result<()> {
    let issues = super::health::run_standard_checks(
        db,
        root_dir,
        only_part,
        deep,
        integrity_only,
        check_files,
    )
    .await?;

    if integrity_only {
        if issues == 0 {
            println!("{}: system integrity reports clean", "check".green());
        }
        return Ok(());
    }

    if issues == 0 {
        let scope = scope_label(only_part);
        let mode = check_mode_label(deep, check_files);
        println!("{}: {} reports clean{}", "check".green(), scope, mode);
        return Ok(());
    }

    let flag = error_flag_label(deep, check_files);
    Err(WrightError::DependencyError(format!(
        "{} issue(s) reported by `wright check{}`",
        issues, flag
    )))
}

fn scope_label(only_part: Option<&str>) -> String {
    match only_part {
        Some(p) => format!("part '{}'", p),
        None => "registry".to_string(),
    }
}

fn check_mode_label(deep: bool, check_files: bool) -> &'static str {
    if deep {
        " (integrity + deps + ELF)"
    } else if check_files {
        " (integrity + files + deps)"
    } else {
        " (integrity + deps)"
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
