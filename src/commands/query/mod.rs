use std::path::Path;

use crate::cli::query::{CheckArgs, DoctorArgs, FilesArgs, HistoryArgs, ListArgs};
use crate::config::GlobalConfig;
use crate::database::InstalledDb;
use crate::error::Result;

pub async fn dispatch_list(args: ListArgs, db_path: &Path) -> Result<()> {
    let db = InstalledDb::open(db_path).await.map_err(|e| {
        crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
    })?;
    crate::operations::list::execute_list(&db, args.long, args.roots, args.assumed, args.orphans)
        .await
}

pub async fn dispatch_files(args: FilesArgs, db_path: &Path) -> Result<()> {
    let db = InstalledDb::open(db_path).await.map_err(|e| {
        crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
    })?;
    crate::operations::files::execute_files(&db, &args.part).await
}

pub async fn dispatch_check(
    args: CheckArgs,
    _config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
) -> Result<()> {
    let db = InstalledDb::open(db_path).await.map_err(|e| {
        crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
    })?;
    crate::operations::check::execute_check(
        &db,
        root_dir,
        args.part.as_deref(),
        args.deep,
        args.integrity_only,
    )
    .await
}

pub async fn dispatch_history(args: HistoryArgs, db_path: &Path) -> Result<()> {
    let db = InstalledDb::open(db_path).await.map_err(|e| {
        crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
    })?;
    crate::operations::history::execute_history(&db, args.part.as_deref()).await
}

pub async fn dispatch_doctor(
    args: DoctorArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
) -> Result<()> {
    let _ = args;
    let db = InstalledDb::open(db_path).await.map_err(|e| {
        crate::error::WrightError::DatabaseError(format!("failed to open database: {}", e))
    })?;
    crate::operations::doctor::execute_doctor(&db, root_dir, config).await
}
