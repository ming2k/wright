pub mod rollback;

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tracing::{info, warn};
use walkdir::WalkDir;

use crate::database::{Database, FileEntry};
use crate::error::{WrightError, Result};
use crate::package::archive::{self, PkgInfo};
use crate::util::checksum;
use crate::repo::source::{SimpleResolver, ResolvedPackage};

use rollback::RollbackState;

/// Install multiple packages with automatic dependency resolution.
pub fn install_packages(
    db: &Database,
    archives: &[PathBuf],
    root_dir: &Path,
    resolver: &SimpleResolver,
) -> Result<()> {
    let mut resolved_map = HashMap::new();
    let mut targets = Vec::new();

    // 1. Load initial archives
    for path in archives {
        let resolved = resolver.read_archive(path)?;
        targets.push(resolved.name.clone());
        resolved_map.insert(resolved.name.clone(), resolved);
    }

    // 2. Recursively resolve dependencies
    let mut queue = targets.clone();
    let mut processed = HashSet::new();

    while let Some(name) = queue.pop() {
        if processed.contains(&name) { continue; }
        
        let dependencies = if let Some(pkg) = resolved_map.get(&name) {
            pkg.dependencies.clone()
        } else {
            continue;
        };

        for dep in &dependencies {
            let (dep_name, _) = crate::package::version::parse_dependency(dep)
                .unwrap_or_else(|_| (dep.clone(), None));
            
            if !resolved_map.contains_key(&dep_name) {
                // Check if already installed
                if db.get_package(&dep_name)?.is_some() {
                    continue;
                }

                // Try to resolve
                if let Some(resolved) = resolver.resolve(&dep_name)? {
                    queue.push(dep_name.clone());
                    resolved_map.insert(dep_name, resolved);
                } else {
                    return Err(WrightError::DependencyError(format!("could not resolve dependency: {}", dep_name)));
                }
            } else {
                queue.push(dep_name);
            }
        }
        processed.insert(name);
    }

    // 3. Build dependency graph for topological sort
    // Simplified topo sort for this context
    let mut sorted_names = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    for name in resolved_map.keys() {
        visit_resolved(name, &resolved_map, &mut visited, &mut visiting, &mut sorted_names)?;
    }

    // 4. Install in order
    for name in sorted_names {
        // Skip if already installed (could have been installed by a previous step in this loop)
        if db.get_package(&name)?.is_some() {
            continue;
        }

        let pkg = resolved_map.get(&name).unwrap();
        info!("Installing {} from {}", name, pkg.path.display());
        install_package(db, &pkg.path, root_dir)?;
    }

    Ok(())
}

fn visit_resolved(
    name: &str,
    map: &HashMap<String, ResolvedPackage>,
    visited: &mut HashSet<String>,
    visiting: &mut HashSet<String>,
    sorted: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(name) { return Ok(()); }
    if visiting.contains(name) {
        return Err(WrightError::DependencyError(format!("circular dependency: {}", name)));
    }

    visiting.insert(name.to_string());

    if let Some(pkg) = map.get(name) {
        for dep in &pkg.dependencies {
            let (dep_name, _) = crate::package::version::parse_dependency(dep)
                .unwrap_or_else(|_| (dep.clone(), None));
            if map.contains_key(&dep_name) {
                visit_resolved(&dep_name, map, visited, visiting, sorted)?;
            }
        }
    }

    visiting.remove(name);
    visited.insert(name.to_string());
    sorted.push(name.to_string());

    Ok(())
}

/// Install a package from a .wright.tar.zst archive.
pub fn install_package(
    db: &Database,
    archive_path: &Path,
    root_dir: &Path,
) -> Result<()> {
    // Extract to temp dir
    let temp_dir = tempfile::tempdir().map_err(|e| {
        WrightError::InstallError(format!("failed to create temp dir: {}", e))
    })?;

    let pkginfo = archive::extract_archive(archive_path, temp_dir.path())?;

    // Check if already installed
    if db.get_package(&pkginfo.name)?.is_some() {
        return Err(WrightError::PackageAlreadyInstalled(pkginfo.name.clone()));
    }

    // Collect file list and check for conflicts
    let file_entries = collect_file_entries(temp_dir.path(), &pkginfo)?;

    for entry in &file_entries {
        if entry.file_type == "file" {
            if let Some(owner) = db.find_owner(&entry.path)? {
                return Err(WrightError::FileConflict {
                    path: PathBuf::from(&entry.path),
                    owner,
                });
            }
        }
    }

    // Begin installation
    let tx_id = db.record_transaction(
        "install",
        &pkginfo.name,
        None,
        Some(&pkginfo.version),
        "pending",
        None,
    )?;

    let mut rollback_state = RollbackState::new();

    // Copy files to root_dir
    match copy_files_to_root(temp_dir.path(), root_dir, &mut rollback_state) {
        Ok(()) => {}
        Err(e) => {
            warn!("Installation failed, rolling back: {}", e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back")?;
            return Err(e);
        }
    }

    // Record in database
    let pkg_hash = checksum::sha256_file(archive_path).ok();
    let pkg_id = db.insert_package(
        &pkginfo.name,
        &pkginfo.version,
        pkginfo.release,
        &pkginfo.description,
        &pkginfo.arch,
        &pkginfo.license,
        None,
        pkginfo.install_size,
        pkg_hash.as_deref(),
    )?;

    db.insert_files(pkg_id, &file_entries)?;

    // Record dependencies
    let deps: Vec<(String, Option<String>)> = pkginfo
        .runtime_deps
        .iter()
        .map(|d| {
            let (name, constraint) = crate::package::version::parse_dependency(d)
                .unwrap_or_else(|_| (d.clone(), None));
            (name, constraint.map(|c| c.to_string()))
        })
        .collect();
    if !deps.is_empty() {
        db.insert_dependencies(pkg_id, &deps)?;
    }

    db.update_transaction_status(tx_id, "completed")?;

    info!("Installed {} {}-{}", pkginfo.name, pkginfo.version, pkginfo.release);
    Ok(())
}

/// Remove an installed package.
pub fn remove_package(
    db: &Database,
    name: &str,
    root_dir: &Path,
) -> Result<()> {
    let pkg = db.get_package(name)?.ok_or_else(|| {
        WrightError::PackageNotFound(name.to_string())
    })?;

    // Check if other packages depend on this one (warn only in Phase 1)
    let dependents = db.get_dependents(name)?;
    if !dependents.is_empty() {
        warn!(
            "Warning: the following packages depend on {}: {}",
            name,
            dependents.join(", ")
        );
    }

    let tx_id = db.record_transaction(
        "remove",
        name,
        Some(&pkg.version),
        None,
        "pending",
        None,
    )?;

    // Get file list
    let files = db.get_files(pkg.id)?;

    // Determine backup files (config files to preserve)
    // For Phase 1, we don't have backup info in DB, so we skip config files marked is_config
    let _backup_files: Vec<&FileEntry> = files.iter().filter(|f| f.is_config).collect();

    // Delete files from root_dir (skip config files)
    for file in files.iter().rev() {
        let full_path = root_dir.join(file.path.trim_start_matches('/'));
        if file.is_config {
            info!("Preserving config file: {}", file.path);
            continue;
        }
        match file.file_type.as_str() {
            "file" | "symlink" => {
                if full_path.exists() {
                    std::fs::remove_file(&full_path).map_err(|e| {
                        WrightError::RemoveError(format!(
                            "failed to remove {}: {}",
                            full_path.display(),
                            e
                        ))
                    })?;
                }
            }
            "dir" => {
                // Only remove empty directories
                if full_path.is_dir() {
                    let _ = std::fs::remove_dir(&full_path);
                }
            }
            _ => {}
        }
    }

    // Remove from database
    db.remove_package(name)?;
    db.update_transaction_status(tx_id, "completed")?;

    info!("Removed {}", name);
    Ok(())
}

/// Verify installed package file integrity.
pub fn verify_package(
    db: &Database,
    name: &str,
    root_dir: &Path,
) -> Result<Vec<String>> {
    let pkg = db.get_package(name)?.ok_or_else(|| {
        WrightError::PackageNotFound(name.to_string())
    })?;

    let files = db.get_files(pkg.id)?;
    let mut issues = Vec::new();

    for file in &files {
        let full_path = root_dir.join(file.path.trim_start_matches('/'));

        if !full_path.exists() {
            issues.push(format!("MISSING: {}", file.path));
            continue;
        }

        if file.file_type == "file" {
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
        }
    }

    Ok(issues)
}

/// Collect file entries from an extracted archive directory.
fn collect_file_entries(extract_dir: &Path, pkginfo: &PkgInfo) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for entry in WalkDir::new(extract_dir).sort_by_file_name() {
        let entry = entry.map_err(|e| {
            WrightError::InstallError(format!("failed to walk directory: {}", e))
        })?;

        let relative = entry
            .path()
            .strip_prefix(extract_dir)
            .unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy().to_string();

        // Skip root dir and metadata files
        if relative_str.is_empty()
            || relative_str.starts_with(".PKGINFO")
            || relative_str.starts_with(".FILELIST")
            || relative_str.starts_with(".INSTALL")
        {
            continue;
        }

        let file_path = format!("/{}", relative_str);
        let metadata = entry.metadata().map_err(|e| {
            WrightError::InstallError(format!("failed to get metadata: {}", e))
        })?;

        let file_type = if metadata.is_dir() {
            "dir"
        } else if metadata.file_type().is_symlink() {
            "symlink"
        } else {
            "file"
        };

        let file_hash = if file_type == "file" {
            checksum::sha256_file(entry.path()).ok()
        } else {
            None
        };

        let is_config = pkginfo
            .backup_files
            .iter()
            .any(|f| f == &file_path);

        entries.push(FileEntry {
            path: file_path,
            file_hash,
            file_type: file_type.to_string(),
            file_mode: Some(metadata.permissions().mode()),
            file_size: if file_type == "file" {
                Some(metadata.len())
            } else {
                None
            },
            is_config,
        });
    }

    Ok(entries)
}

/// Copy files from extracted archive to root directory.
fn copy_files_to_root(
    extract_dir: &Path,
    root_dir: &Path,
    rollback: &mut RollbackState,
) -> Result<()> {
    for entry in WalkDir::new(extract_dir).sort_by_file_name() {
        let entry = entry.map_err(|e| {
            WrightError::InstallError(format!("failed to walk directory: {}", e))
        })?;

        let relative = entry
            .path()
            .strip_prefix(extract_dir)
            .unwrap_or(entry.path());
        let relative_str = relative.to_string_lossy().to_string();

        // Skip root dir and metadata files
        if relative_str.is_empty()
            || relative_str.starts_with(".PKGINFO")
            || relative_str.starts_with(".FILELIST")
            || relative_str.starts_with(".INSTALL")
        {
            continue;
        }

        let dest_path = root_dir.join(&relative_str);

        if entry.file_type().is_dir() {
            if !dest_path.exists() {
                std::fs::create_dir_all(&dest_path).map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to create directory {}: {}",
                        dest_path.display(),
                        e
                    ))
                })?;
                rollback.record_dir_created(dest_path.clone());
            }
        } else {
            // Ensure parent directory exists
            if let Some(parent) = dest_path.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        WrightError::InstallError(format!(
                            "failed to create directory {}: {}",
                            parent.display(),
                            e
                        ))
                    })?;
                }
            }

            std::fs::copy(entry.path(), &dest_path).map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to copy {} to {}: {}",
                    entry.path().display(),
                    dest_path.display(),
                    e
                ))
            })?;

            // Preserve permissions
            if let Ok(metadata) = entry.metadata() {
                let _ = std::fs::set_permissions(&dest_path, metadata.permissions());
            }

            rollback.record_file_created(dest_path);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test() -> (Database, TempDir) {
        let db = Database::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        (db, root)
    }

    fn build_hello_archive() -> PathBuf {
        use crate::builder::Builder;
        use crate::config::GlobalConfig;
        use crate::package::manifest::PackageManifest;

        let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/hello/package.toml");
        let manifest = PackageManifest::from_file(&manifest_path).unwrap();
        let hold_dir = manifest_path.parent().unwrap();

        let mut config = GlobalConfig::default();
        let build_tmp = tempfile::tempdir().unwrap();
        config.build.build_dir = build_tmp.path().to_path_buf();

        let builder = Builder::new(config);
        let result = builder.build(&manifest, hold_dir, None).unwrap();

        let output_dir = tempfile::tempdir().unwrap();
        let archive = crate::package::archive::create_archive(
            &result.pkg_dir,
            &manifest,
            output_dir.path(),
        )
        .unwrap();

        // Copy to a persistent location with unique name
        let persistent = std::env::temp_dir().join(format!(
            "hello-test-{}-{}.wright.tar.zst",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::copy(&archive, &persistent).unwrap();
        persistent
    }

    #[test]
    fn test_install_and_query() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path()).unwrap();

        let pkg = db.get_package("hello").unwrap().unwrap();
        assert_eq!(pkg.name, "hello");
        assert_eq!(pkg.version, "1.0.0");

        // Verify file exists
        assert!(root.path().join("usr/bin/hello").exists());

        // Verify DB has file entry
        let files = db.get_files(pkg.id).unwrap();
        assert!(files.iter().any(|f| f.path == "/usr/bin/hello"));

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_install_and_remove() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path()).unwrap();
        assert!(root.path().join("usr/bin/hello").exists());

        remove_package(&db, "hello", root.path()).unwrap();

        assert!(!root.path().join("usr/bin/hello").exists());
        assert!(db.get_package("hello").unwrap().is_none());

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_install_duplicate_rejected() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path()).unwrap();
        let result = install_package(&db, &archive, root.path());
        assert!(result.is_err());

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_remove_nonexistent() {
        let (db, root) = setup_test();
        let result = remove_package(&db, "nonexistent", root.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_package() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path()).unwrap();

        let issues = verify_package(&db, "hello", root.path()).unwrap();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);

        // Tamper with a file
        std::fs::write(root.path().join("usr/bin/hello"), b"tampered").unwrap();
        let issues = verify_package(&db, "hello", root.path()).unwrap();
        assert!(issues.iter().any(|i| i.contains("MODIFIED")));

        let _ = std::fs::remove_file(&archive);
    }
}
