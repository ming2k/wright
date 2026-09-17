use super::migrations::configure_connection;
use super::schema;
use crate::error::{Result, WrightError};
use crate::lock::ProcessLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) const PART_COLUMNS: &str =
    "id, name, plan_id, installed_at, part_hash, deploy_scripts, origin";

const READER_POOL_CAPACITY: usize = 32;

type WriterJob = Box<dyn FnOnce(&mut rusqlite::Connection) + Send>;

pub struct WriterHandle {
    sender: tokio::sync::mpsc::Sender<WriterJob>,
}

impl WriterHandle {
    pub async fn call<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Box::new(move |conn| {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(conn)));
                let result = match res {
                    Ok(r) => r,
                    Err(_) => Err(WrightError::DatabaseError(
                        "database writer operation panicked".into(),
                    )),
                };
                let _ = ack_tx.send(result);
            }))
            .await
            .map_err(|_| WrightError::DatabaseError("database writer thread is down".into()))?;

        ack_rx
            .await
            .map_err(|_| WrightError::DatabaseError("database writer response dropped".into()))?
    }
}

pub struct InstalledDb {
    pub(crate) writer: Option<Arc<WriterHandle>>,
    pub(crate) reader_semaphore: Arc<tokio::sync::Semaphore>,
    pub(super) _lock: Option<ProcessLock>,
    pub(super) db_path: Option<PathBuf>,
}

fn acquire_lock(db_path: &Path) -> Result<ProcessLock> {
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database");

    crate::lock::acquire_lock(
        &crate::lock::lock_dir_from_db(db_path),
        crate::lock::LockIdentity::Database(file_name),
        crate::lock::LockMode::Exclusive,
    )
    .map_err(|e| WrightError::DatabaseError(e.to_string()))
}

fn open_reader_connection(path: &Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    let _ = conn.pragma_update(None, "query_only", true);
    Ok(conn)
}

impl InstalledDb {
    /// Open the installed-state database, running pending migrations.
    ///
    /// `snapshot_export_dir`: when the pre-migration schema still holds the
    /// legacy `plan_snapshots` table, its rows are exported into this
    /// directory (ADR-0041 layout) before the dropping migration runs.
    /// Export failures abort the open — proceeding would drop the table and
    /// lose the snapshots. Pass `None` (tests, in-memory) to skip.
    pub async fn open(path: &Path, snapshot_export_dir: Option<&Path>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                WrightError::context(
                    format!("failed to create database directory {}", parent.display()),
                    e,
                )
            })?;
        }

        let lock_file = acquire_lock(path)?;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::channel::<WriterJob>(256);

        let path_clone = path.to_path_buf();
        let export_dir = snapshot_export_dir.map(|p| p.to_path_buf());

        std::thread::Builder::new()
            .name("wright-db-writer".into())
            .spawn(move || {
                let mut conn = match rusqlite::Connection::open(&path_clone) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(WrightError::context("failed to open database", e)));
                        return;
                    }
                };

                if let Err(e) = configure_connection(&mut conn) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                if let Some(ref dir) = export_dir
                    && let Err(e) = crate::ledger::export_legacy_plan_snapshots(&conn, dir)
                {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                if let Err(e) = schema::init_db(&mut conn) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                let _ = ready_tx.send(Ok(()));

                while let Some(job) = job_rx.blocking_recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| WrightError::context("failed to spawn database writer thread", e))?;

        ready_rx
            .await
            .map_err(|_| WrightError::DatabaseError("database writer thread died during initialization".into()))??;

        Ok(InstalledDb {
            writer: Some(Arc::new(WriterHandle { sender: job_tx })),
            reader_semaphore: Arc::new(tokio::sync::Semaphore::new(READER_POOL_CAPACITY)),
            _lock: Some(lock_file),
            db_path: Some(path.to_path_buf()),
        })
    }

    pub async fn open_in_memory() -> Result<Self> {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (job_tx, mut job_rx) = tokio::sync::mpsc::channel::<WriterJob>(256);

        std::thread::Builder::new()
            .name("wright-db-writer-mem".into())
            .spawn(move || {
                let mut conn = match rusqlite::Connection::open_in_memory() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ready_tx.send(Err(WrightError::context("failed to open in-memory database", e)));
                        return;
                    }
                };

                if let Err(e) = configure_connection(&mut conn) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                if let Err(e) = schema::init_db(&mut conn) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                let _ = ready_tx.send(Ok(()));

                while let Some(job) = job_rx.blocking_recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| WrightError::context("failed to spawn in-memory database writer thread", e))?;

        ready_rx
            .await
            .map_err(|_| WrightError::DatabaseError("database writer thread died during initialization".into()))??;

        Ok(InstalledDb {
            writer: Some(Arc::new(WriterHandle { sender: job_tx })),
            reader_semaphore: Arc::new(tokio::sync::Semaphore::new(READER_POOL_CAPACITY)),
            _lock: None,
            db_path: None,
        })
    }

    /// Dispatch a mutating operation to the single persistent writer actor thread.
    pub async fn write<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        match self.writer {
            Some(ref writer) => writer.call(f).await,
            None => Err(WrightError::DatabaseError(
                "attempted write operation on read-only database".into(),
            )),
        }
    }

    /// Dispatch a read operation concurrently.
    ///
    /// For file-backed databases, reads run concurrently across Tokio blocking
    /// workers using snapshot-isolated read-only SQLite connections, never
    /// waiting on the writer actor.
    /// For in-memory databases, queries route to the writer thread holding the
    /// in-memory database connection.
    pub async fn read<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        if let Some(ref path) = self.db_path {
            let path = path.clone();
            let permit = self
                .reader_semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| WrightError::DatabaseError("database reader capacity exhausted".into()))?;

            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let conn = open_reader_connection(&path)?;
                f(&conn)
            })
            .await
            .map_err(|e| WrightError::context("database read task failed", e))?
        } else if let Some(ref writer) = self.writer {
            writer.call(move |conn| f(conn)).await
        } else {
            Err(WrightError::DatabaseError(
                "database connection unavailable".into(),
            ))
        }
    }
}
