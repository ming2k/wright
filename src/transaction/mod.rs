mod fs;
mod hooks;
mod install;
mod remove;
pub mod rollback;
mod upgrade;
mod verify;

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::params;
use tracing::debug;

use crate::database::Database;
use crate::error::{Result, WrightError};
use crate::part::part::PartInfo;

pub use hooks::get_hook;
pub use install::{
    install_part, install_part_with_origin, install_parts, install_parts_with_explicit_targets,
};
pub use remove::{
    cascade_remove_list, order_removal_batch, remove_part, remove_part_with_ignored_dependents,
};
pub use upgrade::upgrade_part;
pub use verify::verify_part;

/// Compacts a file path for cleaner logging by replacing middle directories with `...`
/// if the path exceeds a reasonable length threshold.
pub fn compact_path(path: &str) -> String {
    if path.len() <= 45 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 4 {
        return path.to_string();
    }
    let n = parts.len();
    format!("/{}/.../{}/{}", parts[0], parts[n - 2], parts[n - 1])
}

/// Derive journal path from the database path.
pub(super) fn journal_path_from_db(db: &Database) -> Option<PathBuf> {
    db.db_path().map(|p| p.with_extension("journal"))
}

/// Replace provides and conflicts rows for a part (used during upgrade).
pub(super) fn self_replace_provides_conflicts(
    db: &Database,
    pkg_id: i64,
    pkginfo: &PartInfo,
) -> Result<()> {
    db.connection()
        .execute("DELETE FROM provides WHERE part_id = ?1", params![pkg_id])
        .map_err(|e| WrightError::DatabaseError(format!("failed to delete old provides: {}", e)))?;
    db.connection()
        .execute("DELETE FROM conflicts WHERE part_id = ?1", params![pkg_id])
        .map_err(|e| {
            WrightError::DatabaseError(format!("failed to delete old conflicts: {}", e))
        })?;

    if !pkginfo.provides.is_empty() {
        db.insert_provides(pkg_id, &pkginfo.provides)?;
    }
    if !pkginfo.conflicts.is_empty() {
        db.insert_conflicts(pkg_id, &pkginfo.conflicts)?;
    }
    Ok(())
}

pub(super) fn log_debug_timing(operation: &str, package: &str, phase: &str, elapsed: Duration) {
    debug!(
        "{} {}: {} completed in {:.3}s",
        operation,
        package,
        phase,
        elapsed.as_secs_f64()
    );
}

#[cfg(test)]
mod tests {
    use super::hooks::parse_hooks_from_db;
    use super::*;
    use crate::database::FileEntry as DbFileEntry;
    use crate::database::{Database, FileEntry, FileType, NewPart};
    use crate::part::version::{Version, VersionConstraint};
    use crate::util::compress;
    use std::path::Path;
    use tempfile::TempDir;

    use std::collections::HashSet;

    fn setup_test() -> (Database, TempDir) {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        (db, root)
    }

    fn build_hello_part() -> PathBuf {
        use crate::builder::Builder;
        use crate::config::GlobalConfig;
        use crate::plan::manifest::PlanManifest;

        let manifest_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello/plan.toml");
        let mut manifest = PlanManifest::from_file(&manifest_path).unwrap();
        for stage in manifest.lifecycle.values_mut() {
            stage.dockyard = "none".to_string();
        }
        let plan_dir = manifest_path.parent().unwrap();

        let mut config = GlobalConfig::default();
        let build_tmp = tempfile::tempdir().unwrap();
        config.build.build_dir = build_tmp.path().to_path_buf();
        config.build.default_dockyard = "none".to_string();

        let builder = Builder::new(config);
        let extra_env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let result = builder
            .build(
                &manifest,
                plan_dir,
                std::path::Path::new("/"),
                &[],
                false,
                false,
                &extra_env,
                false,
                false,
                None,
                None,
                None,
            )
            .unwrap();

        let output_dir = tempfile::tempdir().unwrap();
        let part =
            crate::part::part::create_part(&result.pkg_dir, &manifest, output_dir.path()).unwrap();

        let persistent = std::env::temp_dir().join(format!(
            "hello-test-{}-{}.wright.tar.zst",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::copy(&part, &persistent).unwrap();
        persistent
    }

    fn build_minimal_part(
        name: &str,
        version: &str,
        release: u32,
        files: &[(&str, &[u8])],
        out_dir: &Path,
    ) -> PathBuf {
        build_part_with_runtime_deps(name, version, release, &[], files, out_dir)
    }

    fn build_part_with_runtime_deps(
        name: &str,
        version: &str,
        release: u32,
        runtime_deps: &[&str],
        files: &[(&str, &[u8])],
        out_dir: &Path,
    ) -> PathBuf {
        let pkg_dir = tempfile::tempdir().unwrap();
        for (rel, data) in files {
            let path = pkg_dir.path().join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, data).unwrap();
        }

        let deps_section = if runtime_deps.is_empty() {
            String::new()
        } else {
            format!(
                "\n[dependencies]\nruntime = [{}]\n",
                runtime_deps
                    .iter()
                    .map(|dep| format!("\"{}\"", dep))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        let pkginfo = format!(
            r#"[part]
name = "{name}"
version = "{version}"
release = {release}
description = "test"
arch = "x86_64"
license = "MIT"
install_size = 0
build_date = "1970-01-01T00:00:00Z"
{deps}"#,
            deps = deps_section,
        );
        std::fs::write(pkg_dir.path().join(".PARTINFO"), pkginfo).unwrap();

        let part_path = out_dir.join(format!("{name}-{version}-{release}.wright.tar.zst"));
        compress::create_tar_zst(pkg_dir.path(), &part_path).unwrap();
        part_path
    }

    #[test]
    fn test_install_and_query() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        install_part(&db, &part, root.path(), false).unwrap();

        let part_info = db.get_part("hello").unwrap().unwrap();
        assert_eq!(part_info.name, "hello");
        assert_eq!(part_info.version, "1.0.0");

        assert!(root.path().join("usr/bin/hello").exists());

        let files = db.get_files(part_info.id).unwrap();
        assert!(files.iter().any(|f| f.path == "/usr/bin/hello"));

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_install_and_remove() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        install_part(&db, &part, root.path(), false).unwrap();
        assert!(root.path().join("usr/bin/hello").exists());

        remove_part(&db, "hello", root.path(), false).unwrap();

        assert!(!root.path().join("usr/bin/hello").exists());
        assert!(db.get_part("hello").unwrap().is_none());

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_install_duplicate_rejected() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        install_part(&db, &part, root.path(), false).unwrap();
        let result = install_part(&db, &part, root.path(), false);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_remove_nonexistent() {
        let (db, root) = setup_test();
        let result = remove_part(&db, "nonexistent", root.path(), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_with_ignored_dependents_allows_same_batch_targets() {
        let (db, root) = setup_test();

        let lib_id = db
            .insert_part(NewPart {
                name: "libfoo",
                version: "1.0.0",
                release: 1,
                description: "libfoo",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();
        db.insert_files(
            lib_id,
            &[FileEntry {
                path: "/usr/lib/libfoo.so".to_string(),
                file_hash: None,
                file_type: FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .unwrap();

        let app_id = db
            .insert_part(NewPart {
                name: "app",
                version: "1.0.0",
                release: 1,
                description: "app",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();
        db.insert_files(
            app_id,
            &[FileEntry {
                path: "/usr/bin/app".to_string(),
                file_hash: None,
                file_type: FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .unwrap();
        db.insert_dependencies(
            app_id,
            &[crate::database::Dependency {
                name: "libfoo".to_string(),
                constraint: None,
                dep_type: crate::database::DepType::Link,
            }],
        )
        .unwrap();

        let err = remove_part(&db, "libfoo", root.path(), false).unwrap_err();
        assert!(format!("{}", err).contains("Cannot remove 'libfoo'"));

        let ignored = HashSet::from([String::from("app")]);
        remove_part_with_ignored_dependents(&db, "libfoo", root.path(), false, &ignored).unwrap();
        assert!(db.get_part("libfoo").unwrap().is_none());
        assert!(db.get_part("app").unwrap().is_some());
    }

    #[test]
    fn test_order_removal_batch_removes_dependents_first() {
        let db = Database::open_in_memory().unwrap();

        db.insert_part(NewPart {
            name: "libfoo",
            version: "1.0.0",
            release: 1,
            description: "libfoo",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();
        let app_id = db
            .insert_part(NewPart {
                name: "app",
                version: "1.0.0",
                release: 1,
                description: "app",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();
        db.insert_dependencies(
            app_id,
            &[crate::database::Dependency {
                name: "libfoo".to_string(),
                constraint: None,
                dep_type: crate::database::DepType::Link,
            }],
        )
        .unwrap();

        let ordered =
            order_removal_batch(&db, &[String::from("libfoo"), String::from("app")]).unwrap();
        assert_eq!(ordered, vec!["app".to_string(), "libfoo".to_string()]);
    }

    #[test]
    fn test_verify_package() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        install_part(&db, &part, root.path(), false).unwrap();

        let issues = verify_part(&db, "hello", root.path()).unwrap();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);

        std::fs::write(root.path().join("usr/bin/hello"), b"tampered").unwrap();
        let issues = verify_part(&db, "hello", root.path()).unwrap();
        assert!(issues.iter().any(|i| i.contains("MODIFIED")));

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_parse_hooks_toml() {
        let content = "[hooks]\npost_install = \"echo hello\"\npre_remove = \"echo bye\"\n";
        let hooks = parse_hooks_from_db(content);
        assert_eq!(hooks.post_install.as_deref(), Some("echo hello"));
        assert_eq!(hooks.pre_remove.as_deref(), Some("echo bye"));
        assert!(hooks.post_upgrade.is_none());
        assert!(hooks.post_remove.is_none());
    }

    #[test]
    fn test_parse_hooks_toml_multiline() {
        let content = "[hooks]\npost_install = \"\"\"\necho hello\necho world\n\"\"\"\n";
        let hooks = parse_hooks_from_db(content);
        let script = hooks.post_install.unwrap();
        assert!(script.contains("echo hello"));
        assert!(script.contains("echo world"));
    }

    #[test]
    fn test_get_hook() {
        let content = "[hooks]\npost_install = \"ldconfig\"\npre_remove = \"systemctl stop foo\"\n";
        assert_eq!(
            get_hook(content, "post_install").as_deref(),
            Some("ldconfig")
        );
        assert_eq!(
            get_hook(content, "pre_remove").as_deref(),
            Some("systemctl stop foo")
        );
        assert!(get_hook(content, "post_upgrade").is_none());
        assert!(get_hook(content, "nonexistent").is_none());
    }

    #[test]
    fn test_version_constraint_check() {
        let db = Database::open_in_memory().unwrap();
        db.insert_part(NewPart {
            name: "libfoo",
            version: "1.0.0",
            release: 1,
            description: "foo lib",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();

        let installed = db.get_part("libfoo").unwrap().unwrap();
        let installed_ver = Version::parse(&installed.version).unwrap();
        let constraint = VersionConstraint::parse(">= 2.0").unwrap();
        assert!(!constraint.satisfies(&installed_ver));

        let constraint2 = VersionConstraint::parse(">= 1.0").unwrap();
        assert!(constraint2.satisfies(&installed_ver));
    }

    #[test]
    fn test_upgrade_same_version_fails() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        install_part(&db, &part, root.path(), false).unwrap();
        let result = upgrade_part(&db, &part, root.path(), false, true);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("not newer"),
            "Expected 'not newer' error, got: {}",
            err_msg
        );

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_upgrade_same_version_force() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        install_part(&db, &part, root.path(), false).unwrap();
        let result = upgrade_part(&db, &part, root.path(), true, true);
        assert!(
            result.is_ok(),
            "Force upgrade should succeed, got: {:?}",
            result
        );

        let pkg = db.get_part("hello").unwrap().unwrap();
        assert_eq!(pkg.version, "1.0.0");
        assert!(root.path().join("usr/bin/hello").exists());

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_upgrade_not_installed() {
        let (db, root) = setup_test();
        let part = build_hello_part();

        let result = upgrade_part(&db, &part, root.path(), false, true);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not installed"));

        let _ = std::fs::remove_file(&part);
    }

    #[test]
    fn test_version_check_rejects_downgrade() {
        let old_ver = Version::parse("2.0.0").unwrap();
        let new_ver = Version::parse("1.0.0").unwrap();
        let new_release: u32 = 2;
        let old_release: u32 = 1;

        let rejected = new_ver < old_ver || (new_ver == old_ver && new_release <= old_release);
        assert!(rejected, "Downgrade 2.0.0-1 -> 1.0.0-2 should be rejected");

        let new_ver2 = Version::parse("2.0.0").unwrap();
        let new_release2: u32 = 2;
        let rejected2 = new_ver2 < old_ver || (new_ver2 == old_ver && new_release2 <= old_release);
        assert!(!rejected2, "Upgrade 2.0.0-1 -> 2.0.0-2 should be accepted");

        let new_ver3 = Version::parse("3.0.0").unwrap();
        let new_release3: u32 = 1;
        let rejected3 = new_ver3 < old_ver || (new_ver3 == old_ver && new_release3 <= old_release);
        assert!(!rejected3, "Upgrade 2.0.0-1 -> 3.0.0-1 should be accepted");
    }

    #[test]
    fn test_upgrade_preserves_shared_files() {
        let (db, root) = setup_test();

        let a_id = db
            .insert_part(NewPart {
                name: "pkgA",
                version: "1.0.0",
                release: 1,
                description: "A",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        let b_id = db
            .insert_part(NewPart {
                name: "pkgB",
                version: "1.0.0",
                release: 1,
                description: "B",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        let shared_path = root.path().join("usr/share/shared.conf");
        std::fs::create_dir_all(shared_path.parent().unwrap()).unwrap();
        std::fs::write(&shared_path, b"shared").unwrap();

        let old_path = root.path().join("usr/bin/oldtool");
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::write(&old_path, b"old").unwrap();

        db.insert_files(
            a_id,
            &[
                DbFileEntry {
                    path: "/usr/share/shared.conf".to_string(),
                    file_hash: None,
                    file_type: crate::database::FileType::File,
                    file_mode: None,
                    file_size: None,
                    is_config: false,
                },
                DbFileEntry {
                    path: "/usr/bin/oldtool".to_string(),
                    file_hash: None,
                    file_type: crate::database::FileType::File,
                    file_mode: None,
                    file_size: None,
                    is_config: false,
                },
            ],
        )
        .unwrap();

        db.insert_files(
            b_id,
            &[DbFileEntry {
                path: "/usr/share/shared.conf".to_string(),
                file_hash: None,
                file_type: crate::database::FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let part = build_minimal_part(
            "pkgA",
            "2.0.0",
            1,
            &[("usr/bin/newtool", b"new")],
            out_dir.path(),
        );

        upgrade_part(&db, &part, root.path(), false, true).unwrap();

        assert!(shared_path.exists(), "shared file should not be deleted");
        assert!(!old_path.exists(), "old-only file should be removed");
        assert!(root.path().join("usr/bin/newtool").exists());
    }

    #[test]
    fn test_install_rollback_restores_overwritten_file() {
        let (db, root) = setup_test();

        let ok_path = root.path().join("usr/bin/ok");
        std::fs::create_dir_all(ok_path.parent().unwrap()).unwrap();
        std::fs::write(&ok_path, b"old").unwrap();

        let bad_parent = root.path().join("usr/share");
        std::fs::write(&bad_parent, b"not a dir").unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let part = build_minimal_part(
            "broken",
            "1.0.0",
            1,
            &[("usr/bin/ok", b"new"), ("usr/share/conf", b"oops")],
            out_dir.path(),
        );

        let result = install_part(&db, &part, root.path(), false);
        assert!(result.is_err());

        let restored = std::fs::read_to_string(&ok_path).unwrap();
        assert_eq!(restored, "old");
    }

    #[test]
    fn test_upgrade_rollback_restores_symlink() {
        let (db, root) = setup_test();

        let pkg_id = db
            .insert_part(NewPart {
                name: "linkpkg",
                version: "1.0.0",
                release: 1,
                description: "symlink test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        let link_path = root.path().join("usr/bin/a_link");
        std::fs::create_dir_all(link_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("target1", &link_path).unwrap();

        db.insert_files(
            pkg_id,
            &[DbFileEntry {
                path: "/usr/bin/a_link".to_string(),
                file_hash: Some("target1".to_string()),
                file_type: crate::database::FileType::Symlink,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .unwrap();

        let bad_parent = root.path().join("usr/z");
        std::fs::write(&bad_parent, b"not a dir").unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let part = build_minimal_part(
            "linkpkg",
            "2.0.0",
            1,
            &[("usr/bin/a_link", b""), ("usr/z/conf", b"oops")],
            out_dir.path(),
        );

        let temp_unpack = tempfile::tempdir().unwrap();
        crate::util::compress::extract_tar_zst(&part, temp_unpack.path()).unwrap();
        let part_link = temp_unpack.path().join("usr/bin/a_link");
        if part_link.exists() || part_link.symlink_metadata().is_ok() {
            let _ = std::fs::remove_file(&part_link);
        }
        std::os::unix::fs::symlink("target2", &part_link).unwrap();
        let rebuilt = out_dir.path().join("linkpkg-2.0.0-1.wright.tar.zst");
        crate::util::compress::create_tar_zst(temp_unpack.path(), &rebuilt).unwrap();

        let result = upgrade_part(&db, &rebuilt, root.path(), false, true);
        assert!(result.is_err());

        let target = std::fs::read_link(&link_path).unwrap();
        assert_eq!(target.to_string_lossy(), "target1");
    }

    #[test]
    fn test_install_parts_with_explicit_targets_marks_dependency_origins() {
        let (db, root) = setup_test();
        let out_dir = tempfile::tempdir().unwrap();

        let lib_part = build_part_with_runtime_deps(
            "libfoo",
            "1.0.0",
            1,
            &[],
            &[("usr/lib/libfoo.so", b"libfoo")],
            out_dir.path(),
        );
        let app_part = build_part_with_runtime_deps(
            "app",
            "1.0.0",
            1,
            &["libfoo"],
            &[("usr/bin/app", b"app")],
            out_dir.path(),
        );

        let mut resolver = crate::inventory::resolver::LocalResolver::new();
        resolver.add_search_dir(out_dir.path().to_path_buf());

        let explicit_targets = HashSet::from(["app".to_string()]);
        install_parts_with_explicit_targets(
            &db,
            &[lib_part, app_part],
            &explicit_targets,
            root.path(),
            &resolver,
            false,
            false,
        )
        .unwrap();

        let app = db.get_part("app").unwrap().unwrap();
        let libfoo = db.get_part("libfoo").unwrap().unwrap();
        assert_eq!(app.origin, crate::database::Origin::Manual);
        assert_eq!(libfoo.origin, crate::database::Origin::Dependency);
    }

    #[test]
    fn test_install_parts_upgrades_when_part_hash_changes() {
        let (db, root) = setup_test();
        let out_dir = tempfile::tempdir().unwrap();

        let first = build_minimal_part(
            "samepkg",
            "1.0.0",
            1,
            &[("usr/bin/samepkg", b"old")],
            out_dir.path(),
        );
        install_part(&db, &first, root.path(), false).unwrap();

        let second = build_minimal_part(
            "samepkg",
            "1.0.0",
            1,
            &[("usr/bin/samepkg", b"new")],
            out_dir.path(),
        );

        let mut resolver = crate::inventory::resolver::LocalResolver::new();
        resolver.add_search_dir(out_dir.path().to_path_buf());

        let explicit_targets = HashSet::from(["samepkg".to_string()]);
        install_parts_with_explicit_targets(
            &db,
            std::slice::from_ref(&second),
            &explicit_targets,
            root.path(),
            &resolver,
            false,
            false,
        )
        .unwrap();

        let installed = db.get_part("samepkg").unwrap().unwrap();
        let expected_hash = crate::util::checksum::sha256_file(&second).unwrap();
        assert_eq!(installed.pkg_hash.as_deref(), Some(expected_hash.as_str()));
        assert_eq!(
            std::fs::read(root.path().join("usr/bin/samepkg")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn test_verify_symlink_detects_change() {
        let (db, root) = setup_test();

        let pkg_id = db
            .insert_part(NewPart {
                name: "linkpkg",
                version: "1.0.0",
                release: 1,
                description: "symlink test",
                arch: "x86_64",
                license: "MIT",
                ..Default::default()
            })
            .unwrap();

        let target1 = root.path().join("usr/bin/target1");
        std::fs::create_dir_all(target1.parent().unwrap()).unwrap();
        std::fs::write(&target1, b"data").unwrap();

        let link_path = root.path().join("usr/bin/mytool");
        std::os::unix::fs::symlink("target1", &link_path).unwrap();

        db.insert_files(
            pkg_id,
            &[FileEntry {
                path: "/usr/bin/mytool".to_string(),
                file_hash: Some("target1".to_string()),
                file_type: crate::database::FileType::Symlink,
                file_mode: None,
                file_size: None,
                is_config: false,
            }],
        )
        .unwrap();

        let issues = verify_part(&db, "linkpkg", root.path()).unwrap();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);

        let target2 = root.path().join("usr/bin/target1-renamed");
        std::fs::write(&target2, b"data").unwrap();
        std::fs::remove_file(&link_path).unwrap();
        std::os::unix::fs::symlink("target1-renamed", &link_path).unwrap();

        let issues = verify_part(&db, "linkpkg", root.path()).unwrap();
        assert!(issues.iter().any(|i| i.contains("MODIFIED")));
    }

    #[test]
    fn test_upgrade_config_existing_file_writes_wnew() {
        let (db, root) = setup_test();

        let conf_rel = "etc/myapp/myapp.conf";
        let conf_v1 = b"setting = default\n";
        let conf_v2 = b"setting = updated\n";

        let make_part = |dir: &std::path::Path,
                         name: &str,
                         ver: &str,
                         content: &[u8],
                         out: &std::path::Path| {
            std::fs::create_dir_all(dir.join("etc/myapp")).unwrap();
            std::fs::write(dir.join(conf_rel), content).unwrap();
            let pkginfo = format!(
                "[part]\nname = \"{name}\"\nversion = \"{ver}\"\nrelease = 1\n\
                 description = \"test\"\narch = \"x86_64\"\nlicense = \"MIT\"\n\
                 install_size = 0\nbuild_date = \"1970-01-01T00:00:00Z\"\n\
                 [backup]\nfiles = [\"/etc/myapp/myapp.conf\"]\n"
            );
            std::fs::write(dir.join(".PARTINFO"), pkginfo).unwrap();
            let part = out.join(format!("{name}-{ver}-1.wright.tar.zst"));
            compress::create_tar_zst(dir, &part).unwrap();
            part
        };

        let out_dir = tempfile::tempdir().unwrap();
        let v1_dir = tempfile::tempdir().unwrap();
        let v1 = make_part(v1_dir.path(), "myapp", "1.0.0", conf_v1, out_dir.path());
        install_part(&db, &v1, root.path(), false).unwrap();

        let live_conf = root.path().join(conf_rel);
        assert_eq!(std::fs::read(&live_conf).unwrap(), conf_v1);

        let v2_dir = tempfile::tempdir().unwrap();
        let v2 = make_part(v2_dir.path(), "myapp", "2.0.0", conf_v2, out_dir.path());
        upgrade_part(&db, &v2, root.path(), false, true).unwrap();

        assert_eq!(
            std::fs::read(&live_conf).unwrap(),
            conf_v1,
            "live config must not be overwritten"
        );
        let wnew = root.path().join(format!("{conf_rel}.wnew"));
        assert!(wnew.exists(), ".wnew must be created");
        assert_eq!(
            std::fs::read(&wnew).unwrap(),
            conf_v2,
            ".wnew must contain the new default"
        );
    }

    #[test]
    fn test_upgrade_config_missing_file_installs_directly() {
        let (db, root) = setup_test();

        let conf_rel = "etc/myapp/myapp.conf";
        let conf_v2 = b"setting = updated\n";

        let make_part = |dir: &std::path::Path,
                         name: &str,
                         ver: &str,
                         include_conf: bool,
                         content: &[u8],
                         out: &std::path::Path| {
            if include_conf {
                std::fs::create_dir_all(dir.join("etc/myapp")).unwrap();
                std::fs::write(dir.join(conf_rel), content).unwrap();
            }
            let backup_section = if include_conf {
                "[backup]\nfiles = [\"/etc/myapp/myapp.conf\"]\n"
            } else {
                ""
            };
            let pkginfo = format!(
                "[part]\nname = \"{name}\"\nversion = \"{ver}\"\nrelease = 1\n\
                 description = \"test\"\narch = \"x86_64\"\nlicense = \"MIT\"\n\
                 install_size = 0\nbuild_date = \"1970-01-01T00:00:00Z\"\n{backup_section}"
            );
            std::fs::write(dir.join(".PARTINFO"), pkginfo).unwrap();
            let part = out.join(format!("{name}-{ver}-1.wright.tar.zst"));
            compress::create_tar_zst(dir, &part).unwrap();
            part
        };

        let out_dir = tempfile::tempdir().unwrap();

        let v1_dir = tempfile::tempdir().unwrap();
        let v1 = make_part(v1_dir.path(), "myapp", "1.0.0", false, b"", out_dir.path());
        install_part(&db, &v1, root.path(), false).unwrap();

        let live_conf = root.path().join(conf_rel);
        assert!(
            !live_conf.exists(),
            "config should be absent before upgrade"
        );

        let v2_dir = tempfile::tempdir().unwrap();
        let v2 = make_part(
            v2_dir.path(),
            "myapp",
            "2.0.0",
            true,
            conf_v2,
            out_dir.path(),
        );
        upgrade_part(&db, &v2, root.path(), false, true).unwrap();

        assert_eq!(
            std::fs::read(&live_conf).unwrap(),
            conf_v2,
            "missing config should be installed directly"
        );
        assert!(
            !root.path().join(format!("{conf_rel}.wnew")).exists(),
            "no .wnew sidecar when config did not previously exist"
        );
    }

    #[test]
    fn test_upgrade_non_config_overwritten_directly() {
        let (db, root) = setup_test();

        let out_dir = tempfile::tempdir().unwrap();

        let v1_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(v1_dir.path().join("usr/bin")).unwrap();
        std::fs::write(v1_dir.path().join("usr/bin/mytool"), b"v1").unwrap();
        let pkginfo_v1 = "[part]\nname = \"mypkg\"\nversion = \"1.0.0\"\nrelease = 1\n\
             description = \"test\"\narch = \"x86_64\"\nlicense = \"MIT\"\n\
             install_size = 0\nbuild_date = \"1970-01-01T00:00:00Z\"\n";
        std::fs::write(v1_dir.path().join(".PARTINFO"), pkginfo_v1).unwrap();
        let v1 = out_dir.path().join("mypkg-1.0.0-1.wright.tar.zst");
        compress::create_tar_zst(v1_dir.path(), &v1).unwrap();
        install_part(&db, &v1, root.path(), false).unwrap();

        let v2_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(v2_dir.path().join("usr/bin")).unwrap();
        std::fs::write(v2_dir.path().join("usr/bin/mytool"), b"v2").unwrap();
        let pkginfo_v2 = "[part]\nname = \"mypkg\"\nversion = \"2.0.0\"\nrelease = 1\n\
             description = \"test\"\narch = \"x86_64\"\nlicense = \"MIT\"\n\
             install_size = 0\nbuild_date = \"1970-01-01T00:00:00Z\"\n";
        std::fs::write(v2_dir.path().join(".PARTINFO"), pkginfo_v2).unwrap();
        let v2 = out_dir.path().join("mypkg-2.0.0-1.wright.tar.zst");
        compress::create_tar_zst(v2_dir.path(), &v2).unwrap();
        upgrade_part(&db, &v2, root.path(), false, true).unwrap();

        let bin = root.path().join("usr/bin/mytool");
        assert_eq!(
            std::fs::read(&bin).unwrap(),
            b"v2",
            "binary must be overwritten"
        );
        assert!(
            !root.path().join("usr/bin/mytool.wnew").exists(),
            "no .wnew for non-config file"
        );
    }
}
