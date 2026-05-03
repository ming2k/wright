mod archive;
mod core;
mod dependencies;
mod files;
mod meta;
mod migrations;
mod parts;
mod plans;
pub mod schema;
mod sessions;
mod types;

pub use archive::{ArchiveDb, ArchivePart};
pub use core::InstalledDb;
use core::PART_COLUMNS;
pub use plans::PlanRecord;
pub use sessions::ExecutionSession;
pub use types::{
    DepType, Dependency, FileEntry, FileType, InstalledPart, NewPart, Origin, TransactionRecord,
};

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_insert_and_get_package() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test part",
                arch: "x86_64",
                license: "MIT",
                install_size: 1024,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(id > 0);

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(part.name, "hello");
        assert_eq!(part.version, "1.0.0");
        assert_eq!(part.release, 1);
        assert_eq!(part.install_size, Some(1024));
        assert!(part.install_scripts.is_none());
    }

    #[tokio::test]
    async fn test_list_packages() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "alpha",
            version: "1.0.0",
            release: 1,
            description: "a",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .await
        .unwrap();
        db.insert_part(NewPart {
            name: "beta",
            version: "2.0.0",
            release: 1,
            description: "b",
            arch: "x86_64",
            license: "MIT",
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
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
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
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
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
        assert!(db.find_owner("/usr/bin/hello").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_insert_and_get_files() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
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
    async fn test_find_owner() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_files(
            id,
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

        assert_eq!(
            db.find_owner("/usr/bin/hello").await.unwrap(),
            Some("hello".to_string())
        );
        assert!(db
            .find_owner("/usr/bin/nonexistent")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_find_owners_batch() {
        let db = test_db().await;
        let hello_id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .await
            .unwrap();
        let world_id = db
            .insert_part(NewPart {
                name: "world",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
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
    async fn test_search_packages() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "Hello World",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .await
        .unwrap();
        db.insert_part(NewPart {
            name: "nginx",
            version: "1.25.3",
            release: 1,
            description: "HTTP server",
            arch: "x86_64",
            license: "BSD",
            ..Default::default()
        })
        .await
        .unwrap();

        let results = db.search_parts("hello").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "hello");

        let results = db.search_parts("server").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "nginx");
    }

    #[tokio::test]
    async fn test_duplicate_package() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .await
        .unwrap();
        let result = db
            .insert_part(NewPart {
                name: "hello",
                version: "2.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
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
            version: "3.0.0",
            release: 1,
            description: "SSL lib",
            arch: "x86_64",
            license: "Apache",
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(db.check_dependency("openssl").await.unwrap());
        assert!(!db.check_dependency("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_record_transaction() {
        let db = test_db().await;
        let id = db
            .record_transaction("install", "hello", None, Some("1.0.0"), "completed", None)
            .await
            .unwrap();
        assert!(id > 0);
        db.update_transaction_status(id, "rolled_back")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_update_package() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test part",
            arch: "x86_64",
            license: "MIT",
            install_size: 1024,
            ..Default::default()
        })
        .await
        .unwrap();

        db.update_part(NewPart {
            name: "hello",
            version: "2.0.0",
            release: 1,
            description: "updated part",
            arch: "x86_64",
            license: "MIT",
            install_size: 2048,
            install_scripts: Some("post_install() { echo hi; }"),
            ..Default::default()
        })
        .await
        .unwrap();

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(part.version, "2.0.0");
        assert_eq!(part.description, Some("updated part".to_string()));
        assert_eq!(part.install_size, Some(2048));
        assert_eq!(
            part.install_scripts.as_deref(),
            Some("post_install() { echo hi; }")
        );
    }

    #[tokio::test]
    async fn test_replace_files() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
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
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .await
            .unwrap();

        db.insert_dependencies(
            id,
            &[Dependency {
                name: "openssl".to_string(),
                version_constraint: Some(">= 3.0".to_string()),
                dep_type: DepType::Runtime,
            }],
        )
        .await
        .unwrap();
        db.replace_dependencies(
            id,
            &[Dependency {
                name: "zlib".to_string(),
                version_constraint: None,
                dep_type: DepType::Runtime,
            }],
        )
        .await
        .unwrap();

        let deps = db.get_dependencies(id).await.unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "zlib");
        assert_eq!(deps[0].dep_type, DepType::Runtime);
    }

    #[tokio::test]
    async fn test_install_scripts_field() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                install_scripts: Some("post_install() { echo done; }"),
                ..Default::default()
            })
            .await
            .unwrap();

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(
            part.install_scripts.as_deref(),
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
    async fn test_plan_name_field() {
        let db = test_db().await;
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test",
                arch: "x86_64",
                license: "MIT",
                plan_name: Some("hello-plan"),
                ..Default::default()
            })
            .await
            .unwrap();

        let part = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(part.plan_name.as_deref(), Some("hello-plan"));

        // update_part should preserve plan_name
        db.update_part(NewPart {
            name: "hello",
            version: "1.0.1",
            release: 2,
            description: "updated",
            arch: "x86_64",
            license: "MIT",
            plan_name: Some("hello-plan-v2"),
            ..Default::default()
        })
        .await
        .unwrap();

        let updated = db.get_part("hello").await.unwrap().unwrap();
        assert_eq!(updated.plan_name.as_deref(), Some("hello-plan-v2"));

        let _ = id;
    }

    #[tokio::test]
    async fn test_get_parts_by_plan() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "gcc",
            version: "14.2.0",
            release: 1,
            description: "compiler",
            arch: "x86_64",
            license: "GPL",
            plan_name: Some("toolchain"),
            ..Default::default()
        })
        .await
        .unwrap();

        db.insert_part(NewPart {
            name: "binutils",
            version: "2.42",
            release: 1,
            description: "binutils",
            arch: "x86_64",
            license: "GPL",
            plan_name: Some("toolchain"),
            ..Default::default()
        })
        .await
        .unwrap();

        db.insert_part(NewPart {
            name: "nginx",
            version: "1.25.0",
            release: 1,
            description: "server",
            arch: "x86_64",
            license: "BSD",
            plan_name: Some("webstack"),
            ..Default::default()
        })
        .await
        .unwrap();

        let toolchain_parts = db.get_parts_by_plan("toolchain").await.unwrap();
        assert_eq!(toolchain_parts.len(), 2);
        assert!(toolchain_parts.iter().any(|p| p.name == "gcc"));
        assert!(toolchain_parts.iter().any(|p| p.name == "binutils"));

        let webstack_parts = db.get_parts_by_plan("webstack").await.unwrap();
        assert_eq!(webstack_parts.len(), 1);
        assert_eq!(webstack_parts[0].name, "nginx");

        let empty = db.get_parts_by_plan("nonexistent").await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_remove_parts_by_plan() {
        let db = test_db().await;
        db.insert_part(NewPart {
            name: "gcc",
            version: "14.2.0",
            release: 1,
            description: "compiler",
            arch: "x86_64",
            license: "GPL",
            plan_name: Some("toolchain"),
            ..Default::default()
        })
        .await
        .unwrap();

        db.insert_part(NewPart {
            name: "binutils",
            version: "2.42",
            release: 1,
            description: "binutils",
            arch: "x86_64",
            license: "GPL",
            plan_name: Some("toolchain"),
            ..Default::default()
        })
        .await
        .unwrap();

        db.insert_part(NewPart {
            name: "nginx",
            version: "1.25.0",
            release: 1,
            description: "server",
            arch: "x86_64",
            license: "BSD",
            plan_name: Some("webstack"),
            ..Default::default()
        })
        .await
        .unwrap();

        let removed = db.remove_parts_by_plan("toolchain").await.unwrap();
        assert_eq!(removed, 2);

        let remaining = db.list_parts().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "nginx");
    }
}
