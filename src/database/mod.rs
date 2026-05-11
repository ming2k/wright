mod core;
mod delivery_db;
mod dependencies;
mod files;
mod meta;
mod migrations;
mod parts;
mod plans;
pub mod schema;
mod types;

pub use core::InstalledDb;
use core::PART_COLUMNS;
pub use plans::PlanRecord;
pub use types::{
    DeliveryStatus, DeliveryTransaction, Dependency, FileEntry, FileType, HistoryAction,
    HistoryRecord, HistoryStatus, InstalledPart, NewPart, NewPlan, OpStatus, Origin, PartWithPlan,
    SessionContext, TransactionOp,
};

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> InstalledDb {
        let db = InstalledDb::open_in_memory().await.unwrap();
        // Insert a default plan so tests can reference it via plan_id
        db.insert_plan(NewPlan {
            name: "test-plan",
            version: "1.0.0",
            release: 1,
            epoch: 0,
            arch: "x86_64",
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn test_insert_and_get_package() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(id > 0);

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(part.name, "hello");
        assert!(part.deploy_scripts.is_none());
    }

    #[tokio::test]
    async fn test_list_packages() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "alpha",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        db.insert_part(NewPart {
            name: "beta",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        let parts = db.list_parts().await.unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "alpha");
        assert_eq!(parts[1].name, "beta");
    }

    #[tokio::test]
    async fn test_remove_package() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "hello",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        db.remove_part("hello").await.unwrap();
        assert!(db.get_part("hello").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_remove_cascades_files() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc123".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            }],
        )
        .await
        .unwrap();

        db.remove_part("hello").await.unwrap();
        let files = db.get_files(id).await.unwrap();
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn test_insert_and_get_files() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        let files = vec![
            FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            },
            FileEntry {
                path: "/usr/share/hello/README".to_string(),
                file_hash: Some("def".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o644),
                file_size: Some(512),
                is_config: false,
            },
        ];
        db.insert_files(id, &files).await.unwrap();

        let result = db.get_files(id).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "/usr/bin/hello");
    }

    #[tokio::test]
    async fn test_find_owners_batch() {
        let db = test_db().await;
        let hello_id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();
        let world_id = db
            .insert_part(NewPart {
                name: "world",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        db.insert_files(
            hello_id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: None,
                file_type: FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .await
        .unwrap();
        db.insert_files(
            world_id,
            &[FileEntry {
                path: "/usr/bin/world".to_string(),
                file_hash: None,
                file_type: FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .await
        .unwrap();

        let owners = db
            .find_owners_batch(&["/usr/bin/hello", "/usr/bin/world", "/usr/bin/missing"])
            .await
            .unwrap();

        assert_eq!(owners.get("/usr/bin/hello"), Some(&"hello".to_string()));
        assert_eq!(owners.get("/usr/bin/world"), Some(&"world".to_string()));
        assert!(!owners.contains_key("/usr/bin/missing"));
    }

    #[tokio::test]
    async fn test_duplicate_package() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "hello",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        let result = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_check_dependency() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "openssl",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(db.check_dependency("openssl").await.unwrap());
        assert!(!db.check_dependency("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_record_history() {
        let db = test_db().await;
        let id = db
            .record_history(
                "session-123",
                "install hello",
                "hello",
                HistoryAction::Install,
                None,
                Some("1.0.0"),
                None,
                None,
                HistoryStatus::Completed,
                None,
            )
            .await
            .unwrap();
        assert!(id > 0);
        db.update_history_status(id, HistoryStatus::RolledBack)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_update_package() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "hello",
            plan_id: 1,
            ..Default::default()
        })
        .await
        .unwrap();

        db.update_part(NewPart {
            name: "hello",
            plan_id: 1,
            deploy_scripts: Some("post_install() { echo hi; }"),
            ..Default::default()
        })
        .await
        .unwrap();

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(
            part.deploy_scripts.as_deref(),
            Some("post_install() { echo hi; }")
        );
    }

    #[tokio::test]
    async fn test_replace_files() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        db.insert_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello".to_string(),
                file_hash: Some("abc".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(1024),
                is_config: false,
            }],
        )
        .await
        .unwrap();

        db.replace_files(
            id,
            &[FileEntry {
                path: "/usr/bin/hello2".to_string(),
                file_hash: Some("def".to_string()),
                file_type: FileType::File,
                file_mode: Some(0o755),
                file_size: Some(2048),
                is_config: false,
            }],
        )
        .await
        .unwrap();

        let files = db.get_files(id).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/bin/hello2");
    }

    #[tokio::test]
    async fn test_replace_dependencies() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        db.insert_dependencies(
            id,
            &[Dependency {
                name: "openssl".to_string(),
                version_constraint: Some(">= 3.0".to_string()),
            }],
        )
        .await
        .unwrap();
        db.replace_dependencies(
            id,
            &[Dependency {
                name: "zlib".to_string(),
                version_constraint: None,
            }],
        )
        .await
        .unwrap();

        let deps = db.get_dependencies(id).await.unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "zlib");
    }

    #[tokio::test]
    async fn test_deploy_scripts_field() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id: 1,
                deploy_scripts: Some("post_install() { echo done; }"),
                ..Default::default()
            })
            .await
            .unwrap();

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(
            part.deploy_scripts.as_deref(),
            Some("post_install() { echo done; }")
        );

        let _ = id;
    }

    #[tokio::test]
    async fn test_database_lock_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let _db1 = InstalledDb::open(&db_path).await.unwrap();
        let result = InstalledDb::open(&db_path).await;
        match result {
            Err(ref e) => {
                let err_msg = format!("{}", e);
                assert!(
                    err_msg.contains("another wright process is already running"),
                    "Expected lock error, got: {}",
                    err_msg
                );
            }
            Ok(_) => panic!("Expected lock error, but open succeeded"),
        }
    }

    #[tokio::test]
    async fn test_plan_id_field() {
        let db = test_db().await;
        let plan_id = db
            .insert_plan(NewPlan {
                name: "hello-plan",
                ..Default::default()
            })
            .await
            .unwrap();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(part.plan_id, plan_id);

        // update_part should preserve plan_id
        let new_plan_id = db
            .insert_plan(NewPlan {
                name: "hello-plan-v2",
                ..Default::default()
            })
            .await
            .unwrap();
        db.update_part(NewPart {
            name: "hello",
            plan_id: new_plan_id,
            ..Default::default()
        })
        .await
        .unwrap();

        let updated = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(updated.plan_id, new_plan_id);

        let _ = id;
    }

    #[tokio::test]
    async fn test_get_parts_by_plan() {
        let db = test_db().await;
        let toolchain_id = db
            .insert_plan(NewPlan {
                name: "toolchain",
                ..Default::default()
            })
            .await
            .unwrap();
        let webstack_id = db
            .insert_plan(NewPlan {
                name: "webstack",
                ..Default::default()
            })
            .await
            .unwrap();

        db.insert_part(NewPart {
            name: "gcc",
            plan_id: toolchain_id,
            ..Default::default()
        })
        .await
        .unwrap();

        db.insert_part(NewPart {
            name: "binutils",
            plan_id: toolchain_id,
            ..Default::default()
        })
        .await
        .unwrap();

        db.insert_part(NewPart {
            name: "nginx",
            plan_id: webstack_id,
            ..Default::default()
        })
        .await
        .unwrap();

        // Parts are inserted for internal testing; plan-level queries are
        // not exposed at the CLI layer to keep the user-facing interface
        // part-centric.
    }
}
