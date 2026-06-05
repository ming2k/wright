use crate::config::GlobalConfig;
use crate::error::Result;
use crate::resolve;

pub async fn execute_lint(
    targets: Vec<String>,
    _recursive: bool,
    verify: bool,
    config: &GlobalConfig,
) -> Result<()> {
    if verify {
        return execute_verify_installed(config).await;
    }

    execute_plan_lint(targets, config).await
}

async fn execute_verify_installed(config: &GlobalConfig) -> Result<()> {
    let t0 = std::time::Instant::now();
    let db_path = config.general.db_path.clone();
    let db = crate::database::InstalledDb::open(&db_path)
        .await
        .map_err(|e| {
            crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
        })?;
    let root_dir = std::path::PathBuf::from("/");

    let parts = db.list_parts().await?;
    crate::cli_action!("Verifying", "{} installed part(s)", parts.len());

    let mut failed: Vec<(String, Vec<String>)> = Vec::new();

    for part in &parts {
        let issues = crate::transaction::verify_part(&db, &part.name, &root_dir).await?;
        if !issues.is_empty() {
            failed.push((part.name.clone(), issues));
        }
    }

    if failed.is_empty() {
        crate::cli_action!(
            "Finished",
            "verify in {}: {} parts clean",
            crate::foundry::logging::format_duration(t0.elapsed().as_secs_f64()),
            parts.len(),
        );
        return Ok(());
    }

    crate::cli_warn!("{} part(s) failed verification", failed.len());
    for (name, issues) in &failed {
        let _ = crate::util::progress::MULTI.println(format!(
            "             - {}: {}",
            name,
            issues.join("; ")
        ));
    }

    Err(crate::error::WrightError::ValidationError(format!(
        "verify failed for {} part(s)",
        failed.len()
    )))
}

async fn execute_plan_lint(targets: Vec<String>, config: &GlobalConfig) -> Result<()> {
    let t0 = std::time::Instant::now();
    crate::cli_action!("Linting", "{} plan(s)", targets.len());
    if let Err(e) = resolve::lint_dependency_graph_for_targets(config, &targets) {
        return Err(crate::error::WrightError::ValidationError(format!(
            "lint failed: {}",
            e
        )));
    }
    crate::cli_action!(
        "Finished",
        "lint in {}: {} plans clean",
        crate::foundry::logging::format_duration(t0.elapsed().as_secs_f64()),
        targets.len(),
    );
    Ok(())
}
