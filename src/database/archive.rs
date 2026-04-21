use std::path::Path;
use sqlx::{query, query_as, SqlitePool, FromRow};
use sqlx::sqlite::SqliteConnectOptions;
use crate::error::{Result, WrightError};
use crate::part::part;
use crate::util::lock::ProcessLock;

#[derive(Debug, Clone, FromRow)]
pub struct ArchivePart {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub release: i64,
    pub epoch: i64,
    pub description: String,
    pub arch: String,
    pub filename: String,
    pub sha256: String,
    pub install_size: i64,
    #[sqlx(skip)]
    pub runtime_deps: Vec<String>,
}

pub struct ArchiveDb {
    pub(crate) pool: SqlitePool,
    _lock: Option<ProcessLock>,
}

fn acquire_lock(db_path: &Path) -> Result<ProcessLock> {
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("database");

    crate::util::lock::acquire_lock(
        &crate::util::lock::lock_dir_from_db(db_path),
        crate::util::lock::LockIdentity::Database(file_name),
        crate::util::lock::LockMode::Exclusive,
    )
    .map_err(|e| WrightError::DatabaseError(e.to_string()))
}

impl ArchiveDb {
    pub async fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                WrightError::DatabaseError(format!("failed to create archive database directory {}: {}", parent.display(), e))
            })?;
        }

        let lock_file = acquire_lock(db_path)?;
        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

        let pool = SqlitePool::connect_with(options).await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to connect to archive database: {}", e))
        })?;

        sqlx::migrate!("./src/database/migrations/archive")
            .run(&pool)
            .await
            .map_err(|e| WrightError::DatabaseError(format!("failed to run archive migrations: {}", e)))?;

        Ok(Self {
            pool,
            _lock: Some(lock_file),
        })
    }

    pub async fn register_part(
        &self,
        partinfo: &part::PartInfo,
        filename: &str,
        sha256: &str,
    ) -> Result<i64> {
        let mut tx = self.pool.begin().await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to begin transaction: {}", e))
        })?;

        query("DELETE FROM parts WHERE name = ? AND version = ? AND release = ? AND epoch = ?")
            .bind(&partinfo.name)
            .bind(&partinfo.version)
            .bind(partinfo.release as i64)
            .bind(partinfo.epoch as i64)
        .execute(&mut *tx)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("delete old entry: {}", e)))?;

        let res = query(
            "INSERT INTO parts (name, version, release, epoch, description, arch, license, filename, sha256, install_size, build_date)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&partinfo.name)
            .bind(&partinfo.version)
            .bind(partinfo.release as i64)
            .bind(partinfo.epoch as i64)
            .bind(&partinfo.description)
            .bind(&partinfo.arch)
            .bind(&partinfo.license)
            .bind(filename)
            .bind(sha256)
            .bind(partinfo.install_size as i64)
            .bind(&partinfo.build_date)
        .execute(&mut *tx)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("insert part: {}", e)))?;

        let part_id = res.last_insert_rowid();

        for dep in &partinfo.runtime_deps {
            query("INSERT INTO dependencies (part_id, depends_on, dep_type) VALUES (?, ?, 'runtime')")
                .bind(part_id)
                .bind(dep)
            .execute(&mut *tx)
            .await?;
        }
        for name in &partinfo.provides {
            query("INSERT INTO provides (part_id, name) VALUES (?, ?)")
                .bind(part_id)
                .bind(name)
            .execute(&mut *tx)
            .await?;
        }
        for name in &partinfo.conflicts {
            query("INSERT INTO conflicts (part_id, name) VALUES (?, ?)")
                .bind(part_id)
                .bind(name)
            .execute(&mut *tx)
            .await?;
        }
        for name in &partinfo.replaces {
            query("INSERT INTO replaces (part_id, name) VALUES (?, ?)")
                .bind(part_id)
                .bind(name)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await.map_err(|e| WrightError::DatabaseError(format!("commit: {}", e)))?;
        Ok(part_id)
    }

    pub async fn list_parts(&self, name: Option<&str>) -> Result<Vec<ArchivePart>> {
        let mut parts = if let Some(name) = name {
            query_as::<_, ArchivePart>(
                "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                 FROM parts WHERE name = ?
                 ORDER BY epoch DESC, version DESC, release DESC",
            )
            .bind(name)
            .fetch_all(&self.pool)
            .await?
        } else {
            query_as::<_, ArchivePart>(
                "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
                 FROM parts ORDER BY name, epoch DESC, version DESC, release DESC",
            )
            .fetch_all(&self.pool)
            .await?
        };

        for part in &mut parts {
            part.runtime_deps = self.get_deps(part.id, "runtime").await?;
        }
        Ok(parts)
    }

    pub async fn find_part(&self, name: &str) -> Result<Option<ArchivePart>> {
        let mut part = query_as::<_, ArchivePart>(
            "SELECT id, name, version, release, epoch, description, arch, filename, sha256, install_size
             FROM parts WHERE name = ?
             ORDER BY epoch DESC, version DESC, release DESC
             LIMIT 1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(ref mut p) = part {
            p.runtime_deps = self.get_deps(p.id, "runtime").await?;
        }
        Ok(part)
    }

    pub async fn find_all_versions(&self, name: &str) -> Result<Vec<ArchivePart>> {
        self.list_parts(Some(name)).await
    }

    pub async fn remove_missing_files(&self, dir: &Path) -> Result<Vec<String>> {
        let filenames = self.list_filenames().await?;
        let mut removed = Vec::new();
        for filename in filenames {
            if !dir.join(&filename).exists() {
                query("DELETE FROM parts WHERE filename = ?")
                    .bind(&filename)
                .execute(&self.pool).await?;
                removed.push(filename);
            }
        }
        Ok(removed)
    }

    pub async fn list_filenames(&self) -> Result<Vec<String>> {
        let rows = query("SELECT filename FROM parts ORDER BY filename").fetch_all(&self.pool).await?;
        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }

    async fn get_deps(&self, part_id: i64, dep_type: &str) -> Result<Vec<String>> {
        let rows = query("SELECT depends_on FROM dependencies WHERE part_id = ? AND dep_type = ?")
            .bind(part_id)
            .bind(dep_type)
        .fetch_all(&self.pool)
        .await?;
        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }
}
