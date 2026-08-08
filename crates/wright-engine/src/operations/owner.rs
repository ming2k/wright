use std::path::{Path, PathBuf};

use crate::error::{Result, WrightError};
use wright_state::database::InstalledDb;

pub async fn execute_owner(db: &InstalledDb, paths: &[PathBuf], json: bool) -> Result<()> {
    let multi = paths.len() > 1;
    let mut any_missing = false;
    let mut results: Vec<serde_json::Value> = Vec::new();

    for input in paths {
        let resolved = normalize_path(input);
        let lookup = resolved.to_string_lossy();

        let owners = db.find_all_owners(&lookup).await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to query owner of {}: {}", lookup, e))
        })?;

        if owners.is_empty() {
            any_missing = true;
            if !json {
                tracing::error!("'{}' is not owned by any deployed part", lookup);
            }
        }

        if json {
            results.push(serde_json::json!({ "path": lookup, "owners": owners }));
        } else {
            for owner in &owners {
                if multi {
                    println!("{}: {}", lookup, owner);
                } else {
                    println!("{}", owner);
                }
            }
        }
    }

    if json {
        super::print_json(&results)?;
    }

    if any_missing {
        return Err(WrightError::ForgeError(
            "one or more paths are not owned by any deployed part".into(),
        ));
    }
    Ok(())
}

fn normalize_path(input: &Path) -> PathBuf {
    let absolute = if input.is_absolute() {
        input.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(input))
            .unwrap_or_else(|_| input.to_path_buf())
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}
