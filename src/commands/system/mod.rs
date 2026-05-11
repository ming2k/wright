pub mod install;

use std::path::Path;

use crate::cli::system::{
    AssumeArgs, InstallArgs, MergeArgs, RemoveArgs, UnassumeArgs, UpgradeArgs,
};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::{Result, WrightError};
use crate::part::store::LocalPartStore;

use crate::util::lock::ProcessLock;

fn ensure_lock_and_part_store(
    db_path: &Path,
    config: &GlobalConfig,
) -> Result<(LocalPartStore, ProcessLock)> {
    let lock = crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(db_path),
        crate::util::lock::LockIdentity::Command("wright"),
        crate::util::lock::LockMode::Exclusive,
    )
    .map_err(|e| WrightError::LockError(format!("failed to start wright operation: {}", e)))?;
    let part_store = crate::commands::setup_local_part_store(config)?;
    Ok((part_store, lock))
}

pub async fn dispatch_merge(
    args: MergeArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
) -> Result<()> {
    let (part_store, _lock) = ensure_lock_and_part_store(db_path, config)?;
    crate::operations::merge::execute_merge(
        args.parts,
        args.force,
        args.nodeps,
        args.path,
        config,
        db_path,
        root_dir,
        &part_store,
    )
    .await
}

pub async fn dispatch_install(
    args: InstallArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let (part_store, _lock) = ensure_lock_and_part_store(db_path, config)?;
    install::execute_system_install(install::InstallArgs {
        targets: args.targets,
        deps: args.deps,
        rdeps: args.rdeps,
        match_policies: args.match_policies,
        depth: args.depth,
        force: args.force,
        dry_run: args.dry_run,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        part_store: &part_store,
    })
    .await
}

pub async fn dispatch_upgrade(
    args: UpgradeArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let (part_store, _lock) = ensure_lock_and_part_store(db_path, config)?;
    crate::operations::upgrade::execute_upgrade(
        args.targets,
        args.force,
        args.depth,
        config,
        db_path,
        root_dir,
        verbose,
        quiet,
        &part_store,
    )
    .await
}

pub async fn dispatch_remove(
    args: RemoveArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
) -> Result<()> {
    let (_, _lock) = ensure_lock_and_part_store(db_path, config)?;
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to open database: {}", e)))?;
    let parts_refs: Vec<&str> = args.parts.iter().map(|s| s.as_str()).collect();
    crate::operations::remove::execute_remove(
        &db,
        &parts_refs,
        args.force,
        args.recursive,
        args.cascade,
        root_dir,
    )
    .await
}

pub async fn dispatch_assume(
    args: AssumeArgs,
    config: &GlobalConfig,
    db_path: &Path,
) -> Result<()> {
    let (_, _lock) = ensure_lock_and_part_store(db_path, config)?;
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to open database: {}", e)))?;
    crate::operations::assume::execute_assume(
        &db,
        args.name.as_deref(),
        args.version.as_deref(),
        args.file.as_deref(),
    )
    .await
}

pub async fn dispatch_unassume(
    args: UnassumeArgs,
    config: &GlobalConfig,
    db_path: &Path,
) -> Result<()> {
    let (_, _lock) = ensure_lock_and_part_store(db_path, config)?;
    let db = InstalledDb::open(db_path)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to open database: {}", e)))?;
    crate::operations::unassume::execute_unassume(&db, &args.name).await
}
