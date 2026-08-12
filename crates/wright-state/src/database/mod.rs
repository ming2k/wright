//! Installed-system SQLite state.

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
    HistoryRecord, HistoryStatus, InstalledPart, NewPart, NewPlan, NewPlanProvenance, OpStatus,
    Origin, PartWithPlan, RegisterPlan, SessionContext, TransactionOp,
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

        // Plan-level queries back the universal plan/output identifier used
        // by `wright files` and `wright remove`; the parts above exercise
        // the lookup across two plans.
        let toolchain_parts = db.get_parts_by_plan("toolchain").await.unwrap();
        assert_eq!(toolchain_parts.len(), 2);
        assert_eq!(toolchain_parts[0].plan_name, "toolchain");
    }

    #[tokio::test]
    async fn test_remove_last_output_cleans_up_multi_output_plan() {
        let db = test_db().await;
        let plan_id = db
            .insert_plan(NewPlan {
                name: "llvm",
                ..Default::default()
            })
            .await
            .unwrap();
        for output in ["clang", "lld"] {
            db.insert_part(NewPart {
                name: output,
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        }

        // Plan row survives while any output remains, and is removed with
        // the last one — keyed by plan_id, not by the part's name.
        db.remove_part("clang").await.unwrap();
        assert!(db.get_plan("llvm").await.unwrap().is_some());
        db.remove_part("lld").await.unwrap();
        assert!(db.get_plan("llvm").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_remove_part_never_touches_same_named_unrelated_plan() {
        let db = test_db().await;
        // Plan `a` deploys an output named `x`; an unrelated plan `x` also
        // exists with its own output `y`.
        let a_id = db
            .insert_plan(NewPlan {
                name: "a",
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: "x",
            plan_id: a_id,
            ..Default::default()
        })
        .await
        .unwrap();
        let x_id = db
            .insert_plan(NewPlan {
                name: "x",
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: "y",
            plan_id: x_id,
            ..Default::default()
        })
        .await
        .unwrap();

        // Removing part `x` (of plan `a`) must clean up plan `a` — which
        // just lost its last output — and leave plan `x` untouched.
        db.remove_part("x").await.unwrap();
        assert!(db.get_plan("a").await.unwrap().is_none());
        assert!(db.get_plan("x").await.unwrap().is_some());
        assert!(db.get_part("y").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_register_plan_persists_archive_independent_provenance() {
        let db = test_db().await;
        let source_checksums = vec!["http https://example.org/src sha256=abc".to_string()];
        let plan_id = db
            .ensure_plan_registered(RegisterPlan {
                plan: NewPlan {
                    name: "provenance-plan",
                    version: "1.2.3",
                    release: 2,
                    epoch: 1,
                    arch: "x86_64",
                },
                provenance: Some(NewPlanProvenance {
                    plan_checksum: Some("deadbeef"),
                    source_checksums: &source_checksums,
                    wright_version: "5.3.12",
                    isolation: "strict",
                }),
            })
            .await
            .unwrap();

        let stored = db.get_plan_by_id(plan_id).await.unwrap().unwrap();
        assert_eq!(stored.name, "provenance-plan");
        assert_eq!(stored.version, "1.2.3");
        assert_eq!(stored.plan_checksum.as_deref(), Some("deadbeef"));
    }

    #[tokio::test]
    async fn test_plan_snapshot_roundtrip_and_dedup() {
        let db = test_db().await;

        assert!(db.get_plan_snapshot("deadbeef").await.unwrap().is_none());

        db.insert_plan_snapshot("deadbeef", "name = \"demo\"\n")
            .await
            .unwrap();
        assert_eq!(
            db.get_plan_snapshot("deadbeef").await.unwrap().as_deref(),
            Some("name = \"demo\"\n")
        );

        // Re-sealing an unchanged plan must not fail or duplicate: the
        // checksum primary key ignores the redundant insert.
        db.insert_plan_snapshot("deadbeef", "name = \"demo\"\n")
            .await
            .unwrap();
        assert_eq!(
            db.get_plan_snapshot("deadbeef").await.unwrap().as_deref(),
            Some("name = \"demo\"\n")
        );
    }

    #[tokio::test]
    async fn test_provide_part_refuses_deployed_plan_name() {
        let db = test_db().await;
        let plan_id = db
            .insert_plan(NewPlan {
                name: "real-plan",
                version: "1.0.0",
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: "real-output",
            plan_id,
            ..Default::default()
        })
        .await
        .unwrap();

        // Providing the name of a plan with a genuinely deployed output would
        // rewrite that plan's version and provenance, so it must be refused —
        // and the plan row must keep its version.
        let result = db.provide_part("real-plan", "9.9.9").await;
        assert!(
            matches!(
                result,
                Err(crate::error::StateError::PartAlreadyInstalled(_))
            ),
            "expected PartAlreadyInstalled, got: {:?}",
            result
        );
        let plan = db.get_plan("real-plan").await.unwrap().unwrap();
        assert_eq!(plan.version, "1.0.0");
    }

    #[tokio::test]
    async fn test_provide_part_allows_external_only_plan_shell() {
        let db = test_db().await;
        db.provide_part("host-lib", "1.0").await.unwrap();
        // Re-providing a name whose plan shell only holds the external
        // placeholder keeps working and updates the version.
        db.provide_part("host-lib", "2.0").await.unwrap();

        let plan = db.get_plan("host-lib").await.unwrap().unwrap();
        assert_eq!(plan.version, "2.0");
        let part = db.get_part("host-lib").await.unwrap().unwrap();
        assert_eq!(part.origin, Origin::External);
    }
}
