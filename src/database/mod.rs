mod core;
mod dependencies;
mod files;
mod meta;
mod parts;
pub mod schema;
mod sessions;
mod types;

pub use core::Database;
use core::{row_to_installed_part, row_to_transaction, PART_COLUMNS};
pub use types::{
    DepType, Dependency, FileEntry, FileType, InstalledPart, NewPart, Origin, TransactionRecord,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    #[test]
    fn test_insert_and_get_package() {
        let db = test_db();
        let id = db
            .insert_part(NewPart {
                name: "hello",
                version: "1.0.0",
                release: 1,
                description: "test pkg",
                arch: "x86_64",
                license: "MIT",
                install_size: 1024,
                ..Default::default()
            })
            .unwrap();
        assert!(id > 0);

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(pkg.name, "hello");
        assert_eq!(pkg.version, "1.0.0");
        assert_eq!(pkg.release, 1);
        assert_eq!(pkg.install_size, 1024);
        assert!(pkg.install_scripts.is_none());
    }

    #[test]
    fn test_list_packages() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "alpha",
            version: "1.0.0",
            release: 1,
            description: "a",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
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
        .unwrap();
        let pkgs = db.list_parts().unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "alpha");
        assert_eq!(pkgs[1].name, "beta");
    }

    #[test]
    fn test_remove_package() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        db.remove_part("hello").unwrap();
        assert!(db.get_part("hello").unwrap().is_none());
    }

    #[test]
    fn test_remove_cascades_files() {
        let db = test_db();
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
        .unwrap();

        db.remove_part("hello").unwrap();
        assert!(db.find_owner("/usr/bin/hello").unwrap().is_none());
    }

    #[test]
    fn test_insert_and_get_files() {
        let db = test_db();
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
        db.insert_files(id, &files).unwrap();

        let result = db.get_files(id).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "/usr/bin/hello");
    }

    #[test]
    fn test_find_owner() {
        let db = test_db();
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
        .unwrap();

        assert_eq!(
            db.find_owner("/usr/bin/hello").unwrap(),
            Some("hello".to_string())
        );
        assert!(db.find_owner("/usr/bin/nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_find_owners_batch() {
        let db = test_db();
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
        .unwrap();

        let owners = db
            .find_owners_batch(&["/usr/bin/hello", "/usr/bin/world", "/usr/bin/missing"])
            .unwrap();

        assert_eq!(owners.get("/usr/bin/hello"), Some(&"hello".to_string()));
        assert_eq!(owners.get("/usr/bin/world"), Some(&"world".to_string()));
        assert!(!owners.contains_key("/usr/bin/missing"));
    }

    #[test]
    fn test_search_packages() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "Hello World",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
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
        .unwrap();

        let results = db.search_parts("hello").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "hello");

        let results = db.search_parts("server").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "nginx");
    }

    #[test]
    fn test_duplicate_package() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        let result = db.insert_part(NewPart {
            name: "hello",
            version: "2.0.0",
            release: 1,
            description: "test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_check_dependency() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "openssl",
            version: "3.0.0",
            release: 1,
            description: "SSL lib",
            arch: "x86_64",
            license: "Apache",
            ..Default::default()
        })
        .unwrap();
        assert!(db.check_dependency("openssl").unwrap());
        assert!(!db.check_dependency("nonexistent").unwrap());
    }

    #[test]
    fn test_record_transaction() {
        let db = test_db();
        let id = db
            .record_transaction("install", "hello", None, Some("1.0.0"), "completed", None)
            .unwrap();
        assert!(id > 0);
        db.update_transaction_status(id, "rolled_back").unwrap();
    }

    #[test]
    fn test_update_package() {
        let db = test_db();
        db.insert_part(NewPart {
            name: "hello",
            version: "1.0.0",
            release: 1,
            description: "test pkg",
            arch: "x86_64",
            license: "MIT",
            install_size: 1024,
            ..Default::default()
        })
        .unwrap();

        db.update_part(NewPart {
            name: "hello",
            version: "2.0.0",
            release: 1,
            description: "updated pkg",
            arch: "x86_64",
            license: "MIT",
            install_size: 2048,
            install_scripts: Some("post_install() { echo hi; }"),
            ..Default::default()
        })
        .unwrap();

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(pkg.version, "2.0.0");
        assert_eq!(pkg.description, "updated pkg");
        assert_eq!(pkg.install_size, 2048);
        assert_eq!(
            pkg.install_scripts.as_deref(),
            Some("post_install() { echo hi; }")
        );
    }

    #[test]
    fn test_replace_files() {
        let db = test_db();
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
        .unwrap();

        let files = db.get_files(id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/usr/bin/hello2");
    }

    #[test]
    fn test_replace_dependencies() {
        let db = test_db();
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
            .unwrap();

        db.insert_dependencies(
            id,
            &[Dependency {
                name: "openssl".to_string(),
                constraint: Some(">= 3.0".to_string()),
                dep_type: DepType::Runtime,
            }],
        )
        .unwrap();
        db.replace_dependencies(
            id,
            &[Dependency {
                name: "zlib".to_string(),
                constraint: None,
                dep_type: DepType::Runtime,
            }],
        )
        .unwrap();

        let deps = db.get_dependencies(id).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "zlib");
        assert_eq!(deps[0].dep_type, DepType::Runtime);
    }

    #[test]
    fn test_install_scripts_field() {
        let db = test_db();
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
            .unwrap();

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(
            pkg.install_scripts.as_deref(),
            Some("post_install() { echo done; }")
        );

        let _ = id;
    }

    #[test]
    fn test_database_lock_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let _db1 = Database::open(&db_path).unwrap();
        let result = Database::open(&db_path);
        match result {
            Err(ref e) => {
                let err_msg = format!("{}", e);
                assert!(
                    err_msg.contains("locked") || err_msg.contains("holding"),
                    "Expected lock error, got: {}",
                    err_msg
                );
            }
            Ok(_) => panic!("Expected lock error, but open succeeded"),
        }
    }
}
