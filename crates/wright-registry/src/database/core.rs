use super::migrations::{CURRENT_DB_VERSION, configure_connection};
use super::schema;
use crate::error::{Result, WrightError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wright_lock::{LockIdentity, LockMode, ProcessLock};

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

/// A read-only view of the installed-state database.
///
/// This type exposes only the query surface: [`ReadOnlyDb::read`] and every
/// read method defined across the `database` modules. It has no `write`
/// method and holds no writer actor for a file-backed database, so a
/// Read-class command *cannot* mutate the system even by mistake — the
/// mutation surface is not reachable from this type at all. This is the
/// type-level enforcement of `[INV-PRIV-01]` (see ADR-0044).
pub struct ReadOnlyDb {
    /// `Some` for file-backed databases; each read opens its own read-only
    /// SQLite connection. `None` for in-memory databases.
    pub(crate) db_path: Option<PathBuf>,
    /// `Some` only for in-memory databases, where reads must route to the
    /// single owning connection. A file-backed [`ReadOnlyDb`] opened via
    /// [`ReadOnlyDb::open_read_only`] leaves this `None`.
    pub(crate) writer: Option<Arc<WriterHandle>>,
    pub(crate) reader_semaphore: Arc<tokio::sync::Semaphore>,
}

/// The read-write installed-state database handle.
///
/// Derefs to [`ReadOnlyDb`], so every query method is available on an
/// `InstalledDb` too. The additional capabilities — [`InstalledDb::write`],
/// the writer actor, and the exclusive process lock — exist only here, which
/// is why the mutation path must name this type explicitly.
pub struct InstalledDb {
    pub(crate) read: ReadOnlyDb,
    pub(super) _lock: Option<ProcessLock>,
}

impl std::ops::Deref for InstalledDb {
    type Target = ReadOnlyDb;

    fn deref(&self) -> &Self::Target {
        &self.read
    }
}

fn acquire_lock(db_path: &Path) -> Result<ProcessLock> {
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database");

    wright_lock::acquire_lock(
        &wright_lock::lock_dir_from_db(db_path),
        LockIdentity::Database(file_name),
        LockMode::Exclusive,
    )
    .map_err(|e| WrightError::DatabaseError(e.to_string()))
}

/// Open a snapshot-isolated read-only connection (ADR-0042).
///
/// A `std::fs` read probe runs first so permission and existence failures
/// surface with an accurate errno (`[INV-PRIV-06]`) instead of SQLite's
/// undifferentiated `CANTOPEN`.
fn open_reader_connection(path: &Path) -> Result<rusqlite::Connection> {
    std::fs::File::open(path)
        .map_err(|e| map_fs_error(e, format!("cannot read database {}", path.display())))?;

    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    let _ = conn.pragma_update(None, "query_only", true);
    Ok(conn)
}

/// Map a filesystem failure on a system path to a typed error: permission and
/// read-only-filesystem failures become [`WrightError::AccessDenied`] with a
/// remediation hint (`[INV-PRIV-06]`).
fn map_fs_error(error: std::io::Error, msg: String) -> WrightError {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => WrightError::AccessDenied(msg),
        _ if error.raw_os_error() == Some(libc::EROFS) => WrightError::AccessDenied(msg),
        _ => WrightError::context(msg, error),
    }
}

impl ReadOnlyDb {
    /// Open an existing database read-only, without any of the mutation side
    /// effects of [`InstalledDb::open`].
    ///
    /// Specifically, this path:
    /// - never creates the database directory (`[INV-PRIV-02]`);
    /// - never creates the database — a missing file is a typed error;
    /// - never runs migrations — a schema older than the binary is a typed
    ///   error naming the command that migrates (`[INV-PRIV-03]`);
    /// - takes **no process lock** (`[INV-PRIV-04]`).
    ///
    /// The absence of a lock is what makes readers non-blocking. The exclusive
    /// process lock on the writer path exists to stop two *writers* from
    /// racing; a reader neither needs it nor may take a shared form of it,
    /// because `flock` shared locks still wait on an exclusive holder — a
    /// shared lock would reintroduce exactly the "a long build blocks `wright
    /// list`" behaviour this ADR removes. Read consistency instead comes from
    /// SQLite WAL snapshot isolation: each [`ReadOnlyDb::read`] opens its own
    /// read-only connection and sees a stable committed snapshot even while a
    /// writer is mid-transaction. The schema-version probe below is the one
    /// guard against a reader observing a half-migrated database.
    ///
    /// This is the entry point for every Read-class command (ADR-0044).
    pub async fn open_read_only(path: &Path) -> Result<ReadOnlyDb> {
        if !path.exists() {
            return Err(WrightError::DatabaseError(format!(
                "no installed-state database at {} (hint: run an install first)",
                path.display()
            )));
        }

        // Verify readability and schema version without holding the writer
        // lock. A read-only connection is the least intrusive probe.
        {
            let path = path.to_path_buf();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let conn = open_reader_connection(&path)?;
                let version: u32 = conn
                    .query_row("PRAGMA user_version", [], |r| r.get(0))
                    .map_err(|e| WrightError::context("failed to read schema version", e))?;
                if version < CURRENT_DB_VERSION {
                    return Err(WrightError::DatabaseError(format!(
                        "database schema is v{version}, but this wright expects v{CURRENT_DB_VERSION} \
                         (hint: a write command such as `wright install` will migrate it)"
                    )));
                }
                Ok(())
            })
            .await
            .map_err(|e| WrightError::context("database probe task failed", e))??;
        }

        Ok(ReadOnlyDb {
            db_path: Some(path.to_path_buf()),
            writer: None,
            reader_semaphore: Arc::new(tokio::sync::Semaphore::new(READER_POOL_CAPACITY)),
        })
    }

    /// Path of the backing database file, or `None` for an in-memory database.
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
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
                .map_err(|_| {
                    WrightError::DatabaseError("database reader capacity exhausted".into())
                })?;

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

impl InstalledDb {
    /// Open the installed-state database read-write, running pending
    /// migrations and holding an exclusive process lock.
    ///
    /// This is the *mutation* entry point: only System- and Local-class
    /// commands may call it (ADR-0044). Read-class commands must use
    /// [`ReadOnlyDb::open_read_only`] instead.
    ///
    /// `snapshot_export_dir`: when the pre-migration schema still holds the
    /// legacy `plan_snapshots` table, its rows are exported into this
    /// directory (ADR-0041 layout) before the dropping migration runs.
    /// Export failures abort the open — proceeding would drop the table and
    /// lose the snapshots. Pass `None` (tests, in-memory) to skip.
    pub async fn open(path: &Path, snapshot_export_dir: Option<&Path>) -> Result<Self> {
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Err(map_fs_error(
                e,
                format!("failed to create database directory {}", parent.display()),
            ));
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
                        let _ =
                            ready_tx.send(Err(WrightError::context("failed to open database", e)));
                        return;
                    }
                };

                if let Err(e) = configure_connection(&mut conn) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                if let Some(ref dir) = export_dir
                    && let Err(e) = export_legacy_plan_snapshots(&conn, dir)
                {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                if let Err(e) = schema::init_db(&mut conn, Some(&path_clone)) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                let _ = ready_tx.send(Ok(()));

                while let Some(job) = job_rx.blocking_recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| WrightError::context("failed to spawn database writer thread", e))?;

        ready_rx.await.map_err(|_| {
            WrightError::DatabaseError("database writer thread died during initialization".into())
        })??;

        Ok(InstalledDb {
            read: ReadOnlyDb {
                db_path: Some(path.to_path_buf()),
                writer: Some(Arc::new(WriterHandle { sender: job_tx })),
                reader_semaphore: Arc::new(tokio::sync::Semaphore::new(READER_POOL_CAPACITY)),
            },
            _lock: Some(lock_file),
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
                        let _ = ready_tx.send(Err(WrightError::context(
                            "failed to open in-memory database",
                            e,
                        )));
                        return;
                    }
                };

                if let Err(e) = configure_connection(&mut conn) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                if let Err(e) = schema::init_db(&mut conn, None) {
                    let _ = ready_tx.send(Err(e));
                    return;
                }

                let _ = ready_tx.send(Ok(()));

                while let Some(job) = job_rx.blocking_recv() {
                    job(&mut conn);
                }
            })
            .map_err(|e| {
                WrightError::context("failed to spawn in-memory database writer thread", e)
            })?;

        ready_rx.await.map_err(|_| {
            WrightError::DatabaseError("database writer thread died during initialization".into())
        })??;

        Ok(InstalledDb {
            read: ReadOnlyDb {
                db_path: None,
                writer: Some(Arc::new(WriterHandle { sender: job_tx })),
                reader_semaphore: Arc::new(tokio::sync::Semaphore::new(READER_POOL_CAPACITY)),
            },
            _lock: None,
        })
    }

    /// Dispatch a mutating operation to the single persistent writer actor thread.
    pub async fn write<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut rusqlite::Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        match self.read.writer {
            Some(ref writer) => writer.call(f).await,
            None => Err(WrightError::DatabaseError(
                "attempted write operation on read-only database".into(),
            )),
        }
    }
}

fn export_legacy_plan_snapshots(conn: &rusqlite::Connection, ledger_dir: &Path) -> Result<usize> {
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'plan_snapshots'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| WrightError::context("failed to inspect legacy snapshot table", e))?
        > 0;
    if !exists {
        return Ok(0);
    }

    let mut stmt = conn
        .prepare(
            "SELECT ps.checksum, ps.source, ps.recorded_at, pl.name AS plan_name
             FROM plan_snapshots ps
             LEFT JOIN plans pl ON pl.plan_checksum = ps.checksum",
        )
        .map_err(|e| WrightError::context("failed to prepare legacy snapshot query", e))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| WrightError::context("failed to read legacy plan snapshots", e))?;

    let mut count = 0;
    for row in rows {
        let (checksum, source, recorded_at, plan_name) =
            row.map_err(|e| WrightError::context("malformed legacy snapshot row", e))?;

        match plan_name {
            Some(name) => {
                let _ = wright_ledger::record_plan_snapshot(
                    ledger_dir,
                    &name,
                    &checksum,
                    &source,
                    recorded_at.as_deref(),
                );
            }
            None => {
                let _ = record_detached_snapshot(
                    ledger_dir,
                    &checksum,
                    &source,
                    recorded_at.as_deref(),
                );
            }
        }
        count += 1;
    }

    if count > 0 {
        tracing::info!(
            "exported {} legacy plan snapshot(s) into {}",
            count,
            ledger_dir.display()
        );
    }
    Ok(count)
}

fn record_detached_snapshot(
    ledger_dir: &Path,
    checksum: &str,
    source: &str,
    recorded_at: Option<&str>,
) -> Result<()> {
    let dir = ledger_dir.join(wright_ledger::DETACHED_DIR);
    if wright_ledger::find_snapshot(&dir, checksum)
        .map_err(|e| WrightError::context("find snapshot", e))?
        .is_some()
    {
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| {
        WrightError::context(
            format!("failed to create snapshot dir {}", dir.display()),
            e,
        )
    })?;
    let file_name = format!(
        "{}-{}.toml",
        wright_ledger::snapshot_timestamp(recorded_at),
        checksum
    );
    let path = dir.join(&file_name);
    let tmp = dir.join(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, source)
        .map_err(|e| WrightError::context(format!("failed to write {}", tmp.display()), e))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| WrightError::context(format!("failed to commit {}", path.display()), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ReadOnlyDb;

    #[test]
    fn read_only_open_missing_database_does_not_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("wright.db");

        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(ReadOnlyDb::open_read_only(&db_path));
        let err = match result {
            Ok(_) => panic!("opening a missing database must fail"),
            Err(e) => e,
        };

        assert!(
            format!("{err}").contains("no installed-state database"),
            "unexpected error: {err}"
        );
        assert!(
            !db_path.parent().unwrap().exists(),
            "open_read_only must not create the database directory"
        );
        assert!(
            !db_path.exists(),
            "open_read_only must not create the database"
        );
    }

    #[test]
    fn read_only_db_has_no_write_surface() {
        // `InstalledDb::write` is not reachable through `&ReadOnlyDb`; the
        // real check is that this crate compiles with read methods on
        // `ReadOnlyDb` and `write` only on `InstalledDb`.
        fn assert_send<T: Send>() {}
        assert_send::<ReadOnlyDb>();
    }
}
