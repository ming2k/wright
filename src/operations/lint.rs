use crate::config::GlobalConfig;
use crate::error::Result;
use crate::resolve;
use tracing::{error, info};

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
    let db_path = config.general.db_path.clone();
    let db = crate::database::InstalledDb::open(&db_path)
        .await
        .map_err(|e| {
            crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
        })?;
    let root_dir = std::path::PathBuf::from("/");

    let parts = db.list_parts().await?;
    let mut all_ok = true;

    for part in &parts {
        let issues = crate::transaction::verify_part(&db, &part.name, &root_dir).await?;
        if issues.is_empty() {
            println!("{}: OK", part.name);
        } else {
            all_ok = false;
            println!("{}:", part.name);
            for issue in &issues {
                println!("  {}", issue);
            }
        }
    }

    if !all_ok {
        return Err(crate::error::WrightError::ValidationError(
            "verify failed: some parts have integrity issues".to_string(),
        ));
    }

    Ok(())
}

async fn execute_plan_lint(targets: Vec<String>, config: &GlobalConfig) -> Result<()> {
    if let Err(e) = resolve::lint_dependency_graph_for_targets(config, &targets) {
        report_lint_error(format!("Dependency graph analysis failed: {}", e));
        return Err(crate::error::WrightError::ValidationError(format!(
            "Lint failed: {}",
            e
        )));
    }
    info!("Lint passed: {} plans.", targets.len());
    Ok(())
}

fn report_lint_error(message: impl AsRef<str>) {
    let message = message.as_ref();
    error!("{}", message);
}
