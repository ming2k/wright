use crate::error::{Result, WrightError};
use crate::identify::{Identifier, ResolvedTarget};
use wright_state::database::{FileEntry, InstalledDb, PartWithPlan};

pub async fn execute_files(db: &InstalledDb, target: &str, json: bool) -> Result<()> {
    let ident = Identifier::parse(target)?;
    let resolved = crate::identify::resolve(db, &ident).await?;

    match &resolved {
        ResolvedTarget::Output { part } => {
            let files = get_files(db, part.id).await?;
            if json {
                let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
                return super::print_json(
                    &serde_json::json!({ "part": part.name, "files": paths }),
                );
            }
            for file in &files {
                crate::outln!("{}", file.path);
            }
        }
        ResolvedTarget::Plan { plan, parts } => {
            let mut per_part: Vec<(&PartWithPlan, Vec<FileEntry>)> =
                Vec::with_capacity(parts.len());
            for part in parts {
                let files = get_files(db, part.id).await?;
                per_part.push((part, files));
            }

            if json {
                let outputs: Vec<serde_json::Value> = per_part
                    .iter()
                    .map(|(part, files)| {
                        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
                        serde_json::json!({ "part": part.name, "files": paths })
                    })
                    .collect();
                return super::print_json(
                    &serde_json::json!({ "plan": plan.name, "outputs": outputs }),
                );
            }

            // A single resolved output prints bare paths (script-friendly);
            // multiple outputs prefix each line with the owning output.
            let prefix = per_part.len() > 1;
            for (part, files) in &per_part {
                for file in files {
                    if prefix {
                        crate::outln!("{}: {}", part.name, file.path);
                    } else {
                        crate::outln!("{}", file.path);
                    }
                }
            }
        }
    }
    Ok(())
}

async fn get_files(db: &InstalledDb, part_id: i64) -> Result<Vec<FileEntry>> {
    db.get_files(part_id)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get files: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WrightError;
    use wright_state::database::{FileType, NewPart, NewPlan};

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

    #[tokio::test]
    async fn files_accepts_plan_and_output_targets() {
        let db = test_db().await;
        add_plan(&db, "llvm", &["clang", "lld"]).await;
        add_plan(&db, "zlib", &["zlib"]).await;

        for target in ["llvm", "llvm:*", "clang", "llvm:clang", "zlib"] {
            execute_files(&db, target, false).await.unwrap();
            execute_files(&db, target, true).await.unwrap();
        }
    }

    #[tokio::test]
    async fn files_rejects_ambiguous_bare_name() {
        let db = test_db().await;
        add_plan(&db, "gcc", &["gcc", "libstdc++"]).await;

        let err = execute_files(&db, "gcc", false).await.unwrap_err();
        assert!(
            matches!(err, WrightError::AmbiguousTarget(_)),
            "expected ambiguous target, got: {}",
            err
        );
    }
}
