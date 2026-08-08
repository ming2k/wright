use crate::error::{Result, WrightError};
use wright_state::database::InstalledDb;

pub async fn execute_files(db: &InstalledDb, part: &str, json: bool) -> Result<()> {
    let installed_part = db
        .get_part(part)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to query part: {}", e)))?;
    let Some(info) = installed_part else {
        return Err(WrightError::PartNotFound(format!(
            "part '{}' is not deployed",
            part
        )));
    };
    let files = db
        .get_files(info.id)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get files: {}", e)))?;

    if json {
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        return super::print_json(&serde_json::json!({ "part": part, "files": paths }));
    }

    for file in &files {
        println!("{}", file.path);
    }
    Ok(())
}
