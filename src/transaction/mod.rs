pub mod rollback;

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::database::{Database, Dependency, DepType, FileEntry, FileType, NewPackage};
use crate::error::{WrightError, Result};
use crate::package::archive::{self, PkgInfo};
use crate::package::version::{self, Version};
use crate::util::checksum;
use crate::repo::source::{SimpleResolver, ResolvedPackage};

use rollback::RollbackState;

// ---------------------------------------------------------------------------
// Install script helpers (Step 5a)
// ---------------------------------------------------------------------------

/// Read the .INSTALL file from an extracted archive directory.
fn read_install_file(extract_dir: &Path) -> Option<String> {
    let path = extract_dir.join(".INSTALL");
    std::fs::read_to_string(&path).ok()
}

/// Parse a `[section]` from .INSTALL content, returning the body text.
pub fn parse_install_section(content: &str, section: &str) -> Option<String> {
    let header = format!("[{}]", section);
    let mut lines = content.lines();
    // Find section header
    loop {
        match lines.next() {
            Some(line) if line.trim() == header => break,
            Some(_) => continue,
            None => return None,
        }
    }
    // Collect body until next section or EOF
    let mut body = String::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            break;
        }
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(line);
    }
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Execute an install script body via /bin/sh.
fn run_install_script(script: &str, root_dir: &Path) -> Result<()> {
    let status = std::process::Command::new("/bin/sh")
        .arg("-e")
        .arg("-c")
        .arg(script)
        .env("ROOT", root_dir)
        .current_dir(root_dir)
        .status()
        .map_err(|e| WrightError::ScriptError(format!("failed to execute script: {}", e)))?;

    if !status.success() {
        return Err(WrightError::ScriptError(format!(
            "script exited with status {}",
            status
        )));
    }
    Ok(())
}

/// Derive journal path from the database path.
fn journal_path_from_db(db: &Database) -> Option<PathBuf> {
    db.db_path().map(|p| p.with_extension("journal"))
}

// ---------------------------------------------------------------------------
// Install flow
// ---------------------------------------------------------------------------

/// Install multiple packages with automatic dependency resolution.
pub fn install_packages(
    db: &Database,
    archives: &[PathBuf],
    root_dir: &Path,
    resolver: &SimpleResolver,
    force: bool,
    nodeps: bool,
) -> Result<()> {
    let mut resolved_map = HashMap::new();
    let mut targets = Vec::new();

    // 1. Load initial archives
    for path in archives {
        let resolved = resolver.read_archive(path)?;
        targets.push(resolved.name.clone());
        resolved_map.insert(resolved.name.clone(), resolved);
    }

    if !nodeps {
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
                let (dep_name, constraint) = version::parse_dependency(dep)
                    .unwrap_or_else(|_| (dep.clone(), None));

                #[allow(clippy::map_entry)]
                if !resolved_map.contains_key(&dep_name) {
                    // Check if already installed
                    if let Some(installed) = db.get_package(&dep_name)? {
                        // Version constraint enforcement (Step 5f)
                        if let Some(ref c) = constraint {
                            let installed_ver = Version::parse(&installed.version)?;
                            if !c.satisfies(&installed_ver) {
                                return Err(WrightError::DependencyError(format!(
                                    "installed {} {} does not satisfy constraint {}",
                                    dep_name, installed.version, c
                                )));
                            }
                        }
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
    }

    // 3. Build dependency graph for topological sort
    let mut sorted_names = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();

    for name in resolved_map.keys() {
        visit_resolved(name, &resolved_map, &mut visited, &mut visiting, &mut sorted_names)?;
    }

    // 4. Install in order
    for name in sorted_names {
        if db.get_package(&name)?.is_some() {
            if force {
                // Force reinstall via upgrade path (atomic — keeps old if new fails)
                info!("Force reinstalling {}", name);
                let pkg = resolved_map.get(&name).unwrap();
                upgrade_package(db, &pkg.path, root_dir, true)?;
            }
            continue;
        }

        let pkg = resolved_map.get(&name).unwrap();
        info!("Installing {} from {}", name, pkg.path.display());
        install_package(db, &pkg.path, root_dir, force)?;
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
            let (dep_name, _) = version::parse_dependency(dep)
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
    force: bool,
) -> Result<()> {
    // Extract to temp dir
    let temp_dir = tempfile::tempdir().map_err(|e| {
        WrightError::InstallError(format!("failed to create temp dir: {}", e))
    })?;

    let pkginfo = archive::extract_archive(archive_path, temp_dir.path())?;

    // --- Handle Replaces (Package Renaming) ---
    for replaced_name in &pkginfo.replaces {
        if db.get_package(replaced_name)?.is_some() {
            info!("Package {} is replaced by {}. Removing {}...", replaced_name, pkginfo.name, replaced_name);
            remove_package(db, replaced_name, root_dir, true)?;
        }
    }

    // --- Handle Conflicts ---
    if !force {
        for conflict_name in &pkginfo.conflicts {
            if db.get_package(conflict_name)?.is_some() {
                return Err(WrightError::DependencyError(format!(
                    "package conflict detected: '{}' conflicts with installed package '{}'. \
                     Please remove it first or use --force.",
                    pkginfo.name, conflict_name
                )));
            }
        }
    }

    // Check if already installed (with same name)
    if db.get_package(&pkginfo.name)?.is_some() {
        if force {
            debug!("Package {} already installed, attempting upgrade/reinstall", pkginfo.name);
            return upgrade_package(db, archive_path, root_dir, true);
        }
        return Err(WrightError::PackageAlreadyInstalled(pkginfo.name.clone()));
    }

    // Read .INSTALL content
    let install_content = read_install_file(temp_dir.path());

    // Collect file list and check for conflicts
    let file_entries = collect_file_entries(temp_dir.path(), &pkginfo)?;

    let mut shadows = Vec::new();
    for entry in &file_entries {
        if entry.file_type == FileType::File {
            if let Some(owner_name) = db.find_owner(&entry.path)? {
                if force {
                    // Don't record self-shadowing (upgrade case)
                    if owner_name != pkginfo.name {
                        warn!("overwriting {} (owned by {})", entry.path, owner_name);
                        shadows.push((entry.path.clone(), owner_name));
                    }
                } else {
                    return Err(WrightError::FileConflict {
                        path: PathBuf::from(&entry.path),
                        owner: owner_name,
                    });
                }
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

    let mut rollback_state = match journal_path_from_db(db) {
        Some(jp) => RollbackState::with_journal(jp),
        None => RollbackState::new(),
    };

    // Backup existing files so install rollback can restore overwrites.
    let backup_dir = tempfile::tempdir().map_err(|e| {
        WrightError::InstallError(format!("failed to create backup dir: {}", e))
    })?;

    // Copy files to root_dir
    match copy_files_to_root(
        temp_dir.path(),
        root_dir,
        &mut rollback_state,
        Some(backup_dir.path()),
    ) {
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
    let pkg_id = db.insert_package(NewPackage {
        name: &pkginfo.name,
        version: &pkginfo.version,
        release: pkginfo.release,
        description: &pkginfo.description,
        arch: &pkginfo.arch,
        license: &pkginfo.license,
        url: None,
        install_size: pkginfo.install_size,
        pkg_hash: pkg_hash.as_deref(),
        install_scripts: install_content.as_deref(),
    })?;

    // Record shadows
    for (path, owner_name) in shadows {
        if let Some(owner_pkg) = db.get_package(&owner_name)? {
            let _ = db.record_shadowed_file(&path, owner_pkg.id, pkg_id);
        }
    }

    db.insert_files(pkg_id, &file_entries)?;

    // Record dependencies
    let mut deps = Vec::new();
    for d in &pkginfo.runtime_deps {
        let (name, constraint) = version::parse_dependency(d)
            .unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency { name, constraint: constraint.map(|c| c.to_string()), dep_type: DepType::Runtime });
    }
    for d in &pkginfo.link_deps {
        let (name, constraint) = version::parse_dependency(d)
            .unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency { name, constraint: constraint.map(|c| c.to_string()), dep_type: DepType::Link });
    }

    if !deps.is_empty() {
        db.insert_dependencies(pkg_id, &deps)?;
    }

    db.update_transaction_status(tx_id, "completed")?;

    // Run post_install hook
    if let Some(ref content) = install_content {
        if let Some(script) = parse_install_section(content, "post_install") {
            debug!("Running post_install hook for {}", pkginfo.name);
            if let Err(e) = run_install_script(&script, root_dir) {
                warn!("post_install script failed: {}", e);
            }
        }
    }

    rollback_state.commit();

    info!("Installed {} {}-{}", pkginfo.name, pkginfo.version, pkginfo.release);
    Ok(())
}

// ---------------------------------------------------------------------------
// Remove flow
// ---------------------------------------------------------------------------

/// Remove an installed package.
///
/// By default, removal is denied if other installed packages depend on this one.
/// Use `force` to override this check.
pub fn remove_package(
    db: &Database,
    name: &str,
    root_dir: &Path,
    force: bool,
) -> Result<()> {
    let pkg = db.get_package(name)?.ok_or_else(|| {
        WrightError::PackageNotFound(name.to_string())
    })?;

    // Check if other packages depend on this one
    let dependents = db.get_dependents(name)?;
    if !dependents.is_empty() {
        let mut link_dependents = Vec::new();
        let mut other_dependents = Vec::new();

        for (dep_name, dep_type) in &dependents {
            if dep_type == "link" {
                link_dependents.push(dep_name.clone());
            } else {
                other_dependents.push(dep_name.clone());
            }
        }

        let all_deps_names: Vec<String> = dependents.iter().map(|(n, _)| n.clone()).collect();
        let deps_str = all_deps_names.join(", ");

        if force {
            warn!(
                "Warning: forcing removal of {} which is depended on by: {}",
                name,
                deps_str
            );
        } else {
            if !link_dependents.is_empty() {
                return Err(WrightError::DependencyError(format!(
                    "CRITICAL: Cannot remove '{}' because it is a LINK dependency of: {}. \
                     Removing it will cause these packages to CRASH. Use --force to override.",
                    name, link_dependents.join(", ")
                )));
            }
            return Err(WrightError::DependencyError(format!(
                "cannot remove '{}': required by {}",
                name,
                deps_str
            )));
        }
    }

    // Run pre_remove hook
    if let Some(ref content) = pkg.install_scripts {
        if let Some(script) = parse_install_section(content, "pre_remove") {
            debug!("Running pre_remove hook for {}", name);
            if let Err(e) = run_install_script(&script, root_dir) {
                warn!("pre_remove script failed (continuing removal): {}", e);
            }
        }
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

    // Delete files from root_dir (skip config files)
    for file in files.iter().rev() {
        let full_path = root_dir.join(file.path.trim_start_matches('/'));
        if file.is_config {
            info!("Preserving config file: {}", file.path);
            continue;
        }

        // --- Multi-ownership check ---
        let other_owners = db.get_other_owners(pkg.id, &file.path)?;
        if !other_owners.is_empty() {
            tracing::debug!(
                "Path {} is also owned by: {}. Skipping deletion.",
                file.path,
                other_owners.join(", ")
            );
            continue;
        }

        match file.file_type {
            FileType::File | FileType::Symlink => {
                if full_path.exists() || full_path.symlink_metadata().is_ok() {
                    std::fs::remove_file(&full_path).map_err(|e| {
                        WrightError::RemoveError(format!(
                            "failed to remove {}: {}",
                            full_path.display(),
                            e
                        ))
                    })?;
                }
            }
            FileType::Directory => {
                // Only remove empty directories
                if full_path.is_dir() {
                    let _ = std::fs::remove_dir(&full_path);
                }
            }
        }
    }

    // Remove from database
    db.remove_package(name)?;
    db.update_transaction_status(tx_id, "completed")?;

    info!("Removed {}", name);
    Ok(())
}

// ---------------------------------------------------------------------------
// Upgrade flow (Step 5g)
// ---------------------------------------------------------------------------

/// Upgrade an installed package to a new version from archive.
pub fn upgrade_package(
    db: &Database,
    archive_path: &Path,
    root_dir: &Path,
    force: bool,
) -> Result<()> {
    // 1. Extract archive and parse .PKGINFO
    let temp_dir = tempfile::tempdir().map_err(|e| {
        WrightError::UpgradeError(format!("failed to create temp dir: {}", e))
    })?;
    let pkginfo = archive::extract_archive(archive_path, temp_dir.path())?;

    // 2. Check old package exists
    let old_pkg = db.get_package(&pkginfo.name)?.ok_or_else(|| {
        WrightError::UpgradeError(format!(
            "package '{}' is not installed, use install instead",
            pkginfo.name
        ))
    })?;

    // 3. Version check: new > old (unless force)
    let old_ver = Version::parse(&old_pkg.version)?;
    let new_ver = Version::parse(&pkginfo.version)?;
    if !force && (new_ver < old_ver || (new_ver == old_ver && pkginfo.release <= old_pkg.release)) {
        return Err(WrightError::UpgradeError(format!(
            "{} {}-{} is not newer than installed {}-{}",
            pkginfo.name,
            pkginfo.version,
            pkginfo.release,
            old_pkg.version,
            old_pkg.release,
        )));
    }

    // Read .INSTALL content
    let install_content = read_install_file(temp_dir.path());

    // 4. Collect new file entries
    let new_entries = collect_file_entries(temp_dir.path(), &pkginfo)?;

    // Check for conflicts with OTHER packages
    for entry in &new_entries {
        if entry.file_type == FileType::File {
            if let Some(owner) = db.find_owner(&entry.path)? {
                if owner != pkginfo.name {
                    if force {
                        warn!("overwriting {} (owned by {})", entry.path, owner);
                    } else {
                        return Err(WrightError::FileConflict {
                            path: PathBuf::from(&entry.path),
                            owner,
                        });
                    }
                }
            }
        }
    }

    // 5. Record upgrade transaction
    let tx_id = db.record_transaction(
        "upgrade",
        &pkginfo.name,
        Some(&old_pkg.version),
        Some(&pkginfo.version),
        "pending",
        None,
    )?;

    let mut rollback_state = match journal_path_from_db(db) {
        Some(jp) => RollbackState::with_journal(jp),
        None => RollbackState::new(),
    };

    // 6. Backup existing files that will be overwritten
    let old_files = db.get_files(old_pkg.id)?;
    let new_paths: HashSet<&str> = new_entries.iter().map(|e| e.path.as_str()).collect();

    let backup_dir = tempfile::tempdir().map_err(|e| {
        WrightError::UpgradeError(format!("failed to create backup dir: {}", e))
    })?;

    for old_file in &old_files {
        if !new_paths.contains(old_file.path.as_str()) {
            continue;
        }

        let full_path = root_dir.join(old_file.path.trim_start_matches('/'));
        if !full_path.exists() && full_path.symlink_metadata().is_err() {
            continue;
        }

        if old_file.file_type == FileType::File {
            let backup_path = backup_dir.path().join(
                old_file.path.trim_start_matches('/'),
            );
            if let Some(parent) = backup_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    WrightError::UpgradeError(format!(
                        "failed to create backup directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
            std::fs::copy(&full_path, &backup_path).map_err(|e| {
                WrightError::UpgradeError(format!(
                    "failed to backup {}: {}",
                    full_path.display(),
                    e
                ))
            })?;
            rollback_state.record_backup(full_path, backup_path);
        } else if old_file.file_type == FileType::Symlink {
            if let Ok(target) = std::fs::read_link(&full_path) {
                rollback_state.record_symlink_backup(
                    full_path,
                    target.to_string_lossy().to_string(),
                );
            }
        }
    }

    // 7. Copy new files to root
    match copy_files_to_root(temp_dir.path(), root_dir, &mut rollback_state, None) {
        Ok(()) => {}
        Err(e) => {
            warn!("Upgrade failed, rolling back: {}", e);
            rollback_state.rollback();
            db.update_transaction_status(tx_id, "rolled_back")?;
            return Err(e);
        }
    }

    // 8. Remove old-only files (files in old but not in new)
    for old_file in old_files.iter().rev() {
        if new_paths.contains(old_file.path.as_str()) {
            continue;
        }

        if old_file.is_config {
            info!("Preserving config file: {}", old_file.path);
            continue;
        }

        let other_owners = db.get_other_owners(old_pkg.id, &old_file.path)?;
        if !other_owners.is_empty() {
            tracing::debug!(
                "Path {} is also owned by: {}. Skipping deletion.",
                old_file.path,
                other_owners.join(", ")
            );
            continue;
        }

        let full_path = root_dir.join(old_file.path.trim_start_matches('/'));
        match old_file.file_type {
            FileType::File | FileType::Symlink => {
                if full_path.exists() || full_path.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&full_path);
                }
            }
            FileType::Directory => {
                let _ = std::fs::remove_dir(&full_path);
            }
        }
    }

    // 9. Update DB
    let pkg_hash = checksum::sha256_file(archive_path).ok();
    db.update_package(NewPackage {
        name: &pkginfo.name,
        version: &pkginfo.version,
        release: pkginfo.release,
        description: &pkginfo.description,
        arch: &pkginfo.arch,
        license: &pkginfo.license,
        url: None,
        install_size: pkginfo.install_size,
        pkg_hash: pkg_hash.as_deref(),
        install_scripts: install_content.as_deref(),
    })?;

    let updated_pkg = db.get_package(&pkginfo.name)?.unwrap();
    db.replace_files(updated_pkg.id, &new_entries)?;

    let mut deps = Vec::new();
    for d in &pkginfo.runtime_deps {
        let (name, constraint) = version::parse_dependency(d)
            .unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency { name, constraint: constraint.map(|c| c.to_string()), dep_type: DepType::Runtime });
    }
    for d in &pkginfo.link_deps {
        let (name, constraint) = version::parse_dependency(d)
            .unwrap_or_else(|_| (d.clone(), None));
        deps.push(Dependency { name, constraint: constraint.map(|c| c.to_string()), dep_type: DepType::Link });
    }
    db.replace_dependencies(updated_pkg.id, &deps)?;

    db.update_transaction_status(tx_id, "completed")?;

    // 10. Run post_upgrade hook
    if let Some(ref content) = install_content {
        if let Some(script) = parse_install_section(content, "post_upgrade") {
            debug!("Running post_upgrade hook for {}", pkginfo.name);
            if let Err(e) = run_install_script(&script, root_dir) {
                warn!("post_upgrade script failed: {}", e);
            }
        }
    }

    // 11. Commit rollback journal
    rollback_state.commit();

    info!(
        "Upgraded {} from {}-{} to {}-{}",
        pkginfo.name,
        old_pkg.version,
        old_pkg.release,
        pkginfo.version,
        pkginfo.release,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// File collection helpers (Step 5b: symlink fix)
// ---------------------------------------------------------------------------

/// Collect file entries from an extracted archive directory.
fn collect_file_entries(extract_dir: &Path, pkginfo: &PkgInfo) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for entry in WalkDir::new(extract_dir).follow_links(false).sort_by_file_name() {
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

        // Use symlink_metadata to avoid following symlinks
        let metadata = entry.path().symlink_metadata().map_err(|e| {
            WrightError::InstallError(format!("failed to get metadata: {}", e))
        })?;

        let file_type = if metadata.is_dir() {
            FileType::Directory
        } else if metadata.file_type().is_symlink() {
            FileType::Symlink
        } else {
            FileType::File
        };

        let file_hash = match file_type {
            FileType::File => checksum::sha256_file(entry.path()).ok(),
            FileType::Symlink => {
                // Store symlink target in file_hash field
                std::fs::read_link(entry.path())
                    .ok()
                    .map(|t| t.to_string_lossy().to_string())
            }
            FileType::Directory => None,
        };

        let is_config = pkginfo
            .backup_files
            .iter()
            .any(|f| f == &file_path);

        entries.push(FileEntry {
            path: file_path,
            file_hash,
            file_size: if file_type == FileType::File {
                Some(metadata.len())
            } else {
                None
            },
            file_type,
            file_mode: Some(metadata.permissions().mode()),
            is_config,
        });
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// File copy helper (Step 5c: symlink fix)
// ---------------------------------------------------------------------------

/// Back up an existing file/symlink at `dest_path` into `backup_root`,
/// recording the action in `rollback`.
fn backup_existing_path(
    dest_path: &Path,
    relative_str: &str,
    backup_root: &Path,
    rollback: &mut RollbackState,
) -> Result<()> {
    let Ok(existing_meta) = dest_path.symlink_metadata() else { return Ok(()); };
    if existing_meta.file_type().is_symlink() {
        if let Ok(target) = std::fs::read_link(dest_path) {
            rollback.record_symlink_backup(dest_path.to_path_buf(), target.to_string_lossy().into());
        }
    } else if existing_meta.is_file() {
        let backup_path = backup_root.join(relative_str);
        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent).map_err(WrightError::IoError)?;
        }
        std::fs::copy(dest_path, &backup_path).map_err(WrightError::IoError)?;
        rollback.record_backup(dest_path.to_path_buf(), backup_path);
    }
    Ok(())
}

/// Copy files from extracted archive to root directory.
fn copy_files_to_root(
    extract_dir: &Path,
    root_dir: &Path,
    rollback: &mut RollbackState,
    backup_dir: Option<&Path>,
) -> Result<()> {
    for entry in WalkDir::new(extract_dir).follow_links(false).sort_by_file_name() {
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

        // Use symlink_metadata to detect type without following
        let metadata = entry.path().symlink_metadata().map_err(|e| {
            WrightError::InstallError(format!("failed to get metadata: {}", e))
        })?;

        if metadata.is_dir() {
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
        } else if metadata.file_type().is_symlink() {
            // Handle symlinks
            let link_target = std::fs::read_link(entry.path()).map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to read symlink {}: {}",
                    entry.path().display(),
                    e
                ))
            })?;

            if let Some(backup_root) = backup_dir {
                backup_existing_path(&dest_path, &relative_str, backup_root, rollback)?;
            }

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

            // Remove existing file/symlink at destination
            if dest_path.symlink_metadata().is_ok() {
                std::fs::remove_file(&dest_path).map_err(|e| {
                    WrightError::InstallError(format!(
                        "failed to remove existing file {}: {}",
                        dest_path.display(),
                        e
                    ))
                })?;
            }

            std::os::unix::fs::symlink(&link_target, &dest_path).map_err(|e| {
                WrightError::InstallError(format!(
                    "failed to create symlink {} -> {}: {}",
                    dest_path.display(),
                    link_target.display(),
                    e
                ))
            })?;

            rollback.record_file_created(dest_path);
        } else {
            // Regular file
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

            if let Some(backup_root) = backup_dir {
                backup_existing_path(&dest_path, &relative_str, backup_root, rollback)?;
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
            if let Err(e) = std::fs::set_permissions(&dest_path, metadata.permissions()) {
                warn!("Failed to set permissions on {}: {}", dest_path.display(), e);
            }

            rollback.record_file_created(dest_path);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::version::VersionConstraint;
    use tempfile::TempDir;
    use crate::util::compress;
    use crate::database::FileEntry as DbFileEntry;

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
            .join("tests/fixtures/hello/plan.toml");
        let mut manifest = PackageManifest::from_file(&manifest_path).unwrap();
        for stage in manifest.lifecycle.values_mut() {
            stage.sandbox = "none".to_string();
        }
        let hold_dir = manifest_path.parent().unwrap();

        let mut config = GlobalConfig::default();
        let build_tmp = tempfile::tempdir().unwrap();
        config.build.build_dir = build_tmp.path().to_path_buf();
        config.build.default_sandbox = "none".to_string();

        let builder = Builder::new(config);
        let extra_env: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let result = builder
            .build(&manifest, hold_dir, None, None, &extra_env, false)
            .unwrap();

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

    fn build_minimal_archive(
        name: &str,
        version: &str,
        release: u32,
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

        let pkginfo = format!(
            r#"[package]
name = "{name}"
version = "{version}"
release = {release}
description = "test"
arch = "x86_64"
license = "MIT"
install_size = 0
build_date = "1970-01-01T00:00:00Z"
"#
        );
        std::fs::write(pkg_dir.path().join(".PKGINFO"), pkginfo).unwrap();

        let archive_path = out_dir.join(format!("{name}-{version}-{release}.wright.tar.zst"));
        compress::create_tar_zst(pkg_dir.path(), &archive_path).unwrap();
        archive_path
    }

    #[test]
    fn test_install_and_query() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path(), false).unwrap();

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

        install_package(&db, &archive, root.path(), false).unwrap();
        assert!(root.path().join("usr/bin/hello").exists());

        remove_package(&db, "hello", root.path(), false).unwrap();

        assert!(!root.path().join("usr/bin/hello").exists());
        assert!(db.get_package("hello").unwrap().is_none());

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_install_duplicate_rejected() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path(), false).unwrap();
        let result = install_package(&db, &archive, root.path(), false);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_remove_nonexistent() {
        let (db, root) = setup_test();
        let result = remove_package(&db, "nonexistent", root.path(), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_package() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path(), false).unwrap();

        let issues = verify_package(&db, "hello", root.path()).unwrap();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);

        // Tamper with a file
        std::fs::write(root.path().join("usr/bin/hello"), b"tampered").unwrap();
        let issues = verify_package(&db, "hello", root.path()).unwrap();
        assert!(issues.iter().any(|i| i.contains("MODIFIED")));

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_parse_install_section_basic() {
        let content = "[post_install]\necho hello\necho world\n[pre_remove]\necho bye\n";
        let post = parse_install_section(content, "post_install").unwrap();
        assert!(post.contains("echo hello"));
        assert!(post.contains("echo world"));
        assert!(!post.contains("echo bye"));

        let pre = parse_install_section(content, "pre_remove").unwrap();
        assert!(pre.contains("echo bye"));

        assert!(parse_install_section(content, "nonexistent").is_none());
    }

    #[test]
    fn test_version_constraint_check() {
        let db = Database::open_in_memory().unwrap();
        // Simulate installed dependency with version 1.0.0
        db.insert_package(NewPackage {
            name: "libfoo",
            version: "1.0.0",
            release: 1,
            description: "foo lib",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        })
        .unwrap();

        // Check that >= 2.0 is NOT satisfied by 1.0.0
        let installed = db.get_package("libfoo").unwrap().unwrap();
        let installed_ver = Version::parse(&installed.version).unwrap();
        let constraint = VersionConstraint::parse(">= 2.0").unwrap();
        assert!(!constraint.satisfies(&installed_ver));

        // Check that >= 1.0 IS satisfied by 1.0.0
        let constraint2 = VersionConstraint::parse(">= 1.0").unwrap();
        assert!(constraint2.satisfies(&installed_ver));
    }

    #[test]
    fn test_upgrade_same_version_fails() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path(), false).unwrap();
        let result = upgrade_package(&db, &archive, root.path(), false);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not newer"), "Expected 'not newer' error, got: {}", err_msg);

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_upgrade_same_version_force() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        install_package(&db, &archive, root.path(), false).unwrap();
        // Force upgrade to same version should succeed
        let result = upgrade_package(&db, &archive, root.path(), true);
        assert!(result.is_ok(), "Force upgrade should succeed, got: {:?}", result);

        // Package should still be installed
        let pkg = db.get_package("hello").unwrap().unwrap();
        assert_eq!(pkg.version, "1.0.0");
        assert!(root.path().join("usr/bin/hello").exists());

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_upgrade_not_installed() {
        let (db, root) = setup_test();
        let archive = build_hello_archive();

        let result = upgrade_package(&db, &archive, root.path(), false);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("not installed"));

        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn test_version_check_rejects_downgrade() {
        // Simulate: installed version 2.0.0-1, trying to "upgrade" to 1.0.0-2
        // Even though release 2 > 1, version 1.0.0 < 2.0.0, so it should be rejected
        let old_ver = Version::parse("2.0.0").unwrap();
        let new_ver = Version::parse("1.0.0").unwrap();
        let new_release: u32 = 2;
        let old_release: u32 = 1;

        // This is the same condition used in upgrade_package
        let rejected = new_ver < old_ver
            || (new_ver == old_ver && new_release <= old_release);
        assert!(rejected, "Downgrade 2.0.0-1 -> 1.0.0-2 should be rejected");

        // Same version, higher release should be accepted
        let new_ver2 = Version::parse("2.0.0").unwrap();
        let new_release2: u32 = 2;
        let rejected2 = new_ver2 < old_ver
            || (new_ver2 == old_ver && new_release2 <= old_release);
        assert!(!rejected2, "Upgrade 2.0.0-1 -> 2.0.0-2 should be accepted");

        // Higher version, lower release should be accepted
        let new_ver3 = Version::parse("3.0.0").unwrap();
        let new_release3: u32 = 1;
        let rejected3 = new_ver3 < old_ver
            || (new_ver3 == old_ver && new_release3 <= old_release);
        assert!(!rejected3, "Upgrade 2.0.0-1 -> 3.0.0-1 should be accepted");
    }

    #[test]
    fn test_upgrade_preserves_shared_files() {
        let (db, root) = setup_test();

        // Package A (to be upgraded)
        let a_id = db.insert_package(NewPackage {
            name: "pkgA",
            version: "1.0.0",
            release: 1,
            description: "A",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        }).unwrap();

        // Package B (shares a file with A)
        let b_id = db.insert_package(NewPackage {
            name: "pkgB",
            version: "1.0.0",
            release: 1,
            description: "B",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        }).unwrap();

        // Create shared file on disk
        let shared_path = root.path().join("usr/share/shared.conf");
        std::fs::create_dir_all(shared_path.parent().unwrap()).unwrap();
        std::fs::write(&shared_path, b"shared").unwrap();

        let old_path = root.path().join("usr/bin/oldtool");
        std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        std::fs::write(&old_path, b"old").unwrap();

        db.insert_files(a_id, &[
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
        ]).unwrap();

        db.insert_files(b_id, &[
            DbFileEntry {
                path: "/usr/share/shared.conf".to_string(),
                file_hash: None,
                file_type: crate::database::FileType::File,
                file_mode: None,
                file_size: None,
                is_config: false,
            },
        ]).unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let archive = build_minimal_archive(
            "pkgA",
            "2.0.0",
            1,
            &[("usr/bin/newtool", b"new")],
            out_dir.path(),
        );

        upgrade_package(&db, &archive, root.path(), false).unwrap();

        assert!(shared_path.exists(), "shared file should not be deleted");
        assert!(!old_path.exists(), "old-only file should be removed");
        assert!(root.path().join("usr/bin/newtool").exists());
    }

    #[test]
    fn test_install_rollback_restores_overwritten_file() {
        let (db, root) = setup_test();

        // Existing file that will be overwritten by install
        let ok_path = root.path().join("usr/bin/ok");
        std::fs::create_dir_all(ok_path.parent().unwrap()).unwrap();
        std::fs::write(&ok_path, b"old").unwrap();

        // Create a file that will cause a later failure (parent is a file)
        let bad_parent = root.path().join("usr/share");
        std::fs::write(&bad_parent, b"not a dir").unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let archive = build_minimal_archive(
            "broken",
            "1.0.0",
            1,
            &[
                ("usr/bin/ok", b"new"),
                ("usr/share/conf", b"oops"),
            ],
            out_dir.path(),
        );

        let result = install_package(&db, &archive, root.path(), false);
        assert!(result.is_err());

        // Rollback should restore original content
        let restored = std::fs::read_to_string(&ok_path).unwrap();
        assert_eq!(restored, "old");
    }

    #[test]
    fn test_upgrade_rollback_restores_symlink() {
        let (db, root) = setup_test();

        let pkg_id = db.insert_package(NewPackage {
            name: "linkpkg",
            version: "1.0.0",
            release: 1,
            description: "symlink test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        }).unwrap();

        let link_path = root.path().join("usr/bin/a_link");
        std::fs::create_dir_all(link_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("target1", &link_path).unwrap();

        db.insert_files(pkg_id, &[
            DbFileEntry {
                path: "/usr/bin/a_link".to_string(),
                file_hash: Some("target1".to_string()),
                file_type: crate::database::FileType::Symlink,
                file_mode: None,
                file_size: None,
                is_config: false,
            },
        ]).unwrap();

        // Create a path that will cause failure on second file
        let bad_parent = root.path().join("usr/z");
        std::fs::write(&bad_parent, b"not a dir").unwrap();

        let out_dir = tempfile::tempdir().unwrap();
        let archive = build_minimal_archive(
            "linkpkg",
            "2.0.0",
            1,
            &[
                ("usr/bin/a_link", b""),
                ("usr/z/conf", b"oops"),
            ],
            out_dir.path(),
        );

        // Replace symlink in the archive to point to target2
        let temp_unpack = tempfile::tempdir().unwrap();
        crate::util::compress::extract_tar_zst(&archive, temp_unpack.path()).unwrap();
        let archive_link = temp_unpack.path().join("usr/bin/a_link");
        if archive_link.exists() || archive_link.symlink_metadata().is_ok() {
            let _ = std::fs::remove_file(&archive_link);
        }
        std::os::unix::fs::symlink("target2", &archive_link).unwrap();
        let rebuilt = out_dir.path().join("linkpkg-2.0.0-1.wright.tar.zst");
        crate::util::compress::create_tar_zst(temp_unpack.path(), &rebuilt).unwrap();

        let result = upgrade_package(&db, &rebuilt, root.path(), false);
        assert!(result.is_err());

        let target = std::fs::read_link(&link_path).unwrap();
        assert_eq!(target.to_string_lossy(), "target1");
    }

    #[test]
    fn test_verify_symlink_detects_change() {
        let (db, root) = setup_test();

        let pkg_id = db.insert_package(NewPackage {
            name: "linkpkg",
            version: "1.0.0",
            release: 1,
            description: "symlink test",
            arch: "x86_64",
            license: "MIT",
            ..Default::default()
        }).unwrap();

        // Create symlink target and symlink itself
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
        ).unwrap();

        let issues = verify_package(&db, "linkpkg", root.path()).unwrap();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);

        // Change symlink target
        let target2 = root.path().join("usr/bin/target1-renamed");
        std::fs::write(&target2, b"data").unwrap();
        std::fs::remove_file(&link_path).unwrap();
        std::os::unix::fs::symlink("target1-renamed", &link_path).unwrap();

        let issues = verify_package(&db, "linkpkg", root.path()).unwrap();
        assert!(issues.iter().any(|i| i.contains("MODIFIED")));
    }
}
