use std::path::Path;

use crate::database::{FileType, InstalledDb};
use crate::error::{Result, WrightError};
use crate::util::checksum;

pub fn verify_part(db: &InstalledDb, name: &str, root_dir: &Path) -> Result<Vec<String>> {
    let pkg = db
        .get_part(name)?
        .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;

    let files = db.get_files(pkg.id)?;
    let mut issues = Vec::new();

    for file in &files {
        let full_path = root_dir.join(file.path.trim_start_matches('/'));

        if !full_path.exists() {
            issues.push(format!("MISSING: {}", file.path));
            continue;
        }

        if file.file_type == FileType::File {
            if let Some(ref expected_hash) = file.file_hash {
                match checksum::sha256_file(&full_path) {
                    Ok(actual_hash) => {
                        if &actual_hash != expected_hash {
                            issues.push(format!("MODIFIED: {}", file.path));
                        }
                    }
                    Err(_) => {
                        issues.push(format!("UNREADABLE: {}", file.path));
                    }
                }
            }
        } else if file.file_type == FileType::Symlink {
            if let Some(ref expected_target) = file.file_hash {
                match std::fs::read_link(&full_path) {
                    Ok(actual_target) => {
                        let actual_str = actual_target.to_string_lossy();
                        if &actual_str != expected_target {
                            issues.push(format!("MODIFIED: {}", file.path));
                        }
                    }
                    Err(_) => {
                        issues.push(format!("UNREADABLE: {}", file.path));
                    }
                }
            }
        }
    }

    Ok(issues)
}
