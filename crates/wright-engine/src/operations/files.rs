use crate::database::InstalledDb;
use crate::error::{Result, WrightError};

pub async fn execute_files(db: &InstalledDb, part: &str) -> Result<()> {
    let installed_part = db
        .get_part(part)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to query part: {}", e)))?;
    match installed_part {
        Some(info) => {
            let files = db
                .get_files(info.id)
                .await
                .map_err(|e| WrightError::DatabaseError(format!("failed to get files: {}", e)))?;
            for file in &files {
                println!("{}", file.path);
            }
        }
        None => {
            tracing::error!("part '{}' is not deployed", part);
            std::process::exit(1);
        }
    }
    Ok(())
}
