use std::collections::HashMap;
use crate::error::{Result, WrightError};
use sqlx::{query, query_as, QueryBuilder, Sqlite};
use super::{FileEntry, InstalledDb};

impl InstalledDb {
    pub async fn record_shadowed_file(
        &self,
        path: &str,
        original_owner_id: i64,
        shadowed_by_id: i64,
        diverted_to: Option<&str>,
    ) -> Result<()> {
        query("INSERT INTO shadowed_files (path, original_owner_id, shadowed_by_id, diverted_to) VALUES (?, ?, ?, ?)")
            .bind(path)
            .bind(original_owner_id)
            .bind(shadowed_by_id)
            .bind(diverted_to)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to record shadowed file: {}", e)))?;
        Ok(())
    }

    pub async fn insert_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to begin transaction: {}", e))
        })?;

        for chunk in files.chunks(999 / 7) {
            let mut query_builder: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO files (part_id, path, file_hash, file_type, file_mode, file_size, is_config) "
            );

            query_builder.push_values(chunk, |mut b, file: &FileEntry| {
                b.push_bind(part_id)
                    .push_bind(&file.path)
                    .push_bind(&file.file_hash)
                    .push_bind(file.file_type)
                    .push_bind(file.file_mode)
                    .push_bind(file.file_size)
                    .push_bind(file.is_config);
            });

            let query = query_builder.build();
            query.execute(&mut *tx).await.map_err(|e| {
                WrightError::DatabaseError(format!("failed to insert files: {}", e))
            })?;
        }

        tx.commit().await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to commit files: {}", e))
        })?;
        Ok(())
    }

    pub async fn get_other_owners(&self, current_part_id: i64, path: &str) -> Result<Vec<String>> {
        let rows = query("SELECT p.name FROM parts p JOIN files f ON p.id = f.part_id WHERE f.path = ? AND p.id != ?")
            .bind(path)
            .bind(current_part_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get other owners: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            result.push(row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?);
        }
        Ok(result)
    }

    pub async fn get_diverted_file(&self, path: &str, shadowed_by_id: i64) -> Result<Option<String>> {
        let row = query("SELECT diverted_to FROM shadowed_files WHERE path = ? AND shadowed_by_id = ?")
            .bind(path)
            .bind(shadowed_by_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to query diverted file: {}", e)))?;
        
        match row {
            Some(r) => {
                use sqlx::Row;
                Ok(r.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?)
            }
            None => Ok(None),
        }
    }

    pub async fn get_all_diverted_files(&self, shadowed_by_id: i64) -> Result<Vec<(String, String)>> {
        let rows = query("SELECT path, diverted_to FROM shadowed_files WHERE shadowed_by_id = ? AND diverted_to IS NOT NULL")
            .bind(shadowed_by_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get all diverted files: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            let path: String = row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let div: String = row.try_get(1).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            result.push((path, div));
        }
        Ok(result)
    }

    pub async fn remove_shadowed_records(&self, shadowed_by_id: i64) -> Result<()> {
        query("DELETE FROM shadowed_files WHERE shadowed_by_id = ?")
            .bind(shadowed_by_id)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to remove shadowed records: {}", e)))?;
        Ok(())
    }

    pub async fn replace_files(&self, part_id: i64, files: &[FileEntry]) -> Result<()> {
        query("DELETE FROM files WHERE part_id = ?")
            .bind(part_id)
        .execute(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to delete old files: {}", e)))?;
        
        self.insert_files(part_id, files).await
    }

    pub async fn get_files(&self, part_id: i64) -> Result<Vec<FileEntry>> {
        query_as::<_, FileEntry>(
            "SELECT path, file_hash, file_type, file_mode, file_size, is_config
             FROM files WHERE part_id = ? ORDER BY path")
            .bind(part_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get files: {}", e)))
    }

    pub async fn find_owner(&self, path: &str) -> Result<Option<String>> {
        let row = query("SELECT p.name FROM files f JOIN parts p ON f.part_id = p.id WHERE f.path = ?")
            .bind(path)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to find owner: {}", e)))?;
        
        match row {
            Some(r) => {
                use sqlx::Row;
                Ok(r.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?)
            }
            None => Ok(None),
        }
    }

    pub async fn find_owners_batch(&self, paths: &[&str]) -> Result<HashMap<String, String>> {
        let mut result = HashMap::new();
        for chunk in paths.chunks(999) {
            let mut query_builder: QueryBuilder<Sqlite> = QueryBuilder::new(
                "SELECT f.path, p.name FROM files f JOIN parts p ON f.part_id = p.id WHERE f.path IN ("
            );
            
            let mut separated = query_builder.separated(", ");
            for path in chunk {
                separated.push_bind(path);
            }
            separated.push_unseparated(")");

            let rows: Vec<sqlx::sqlite::SqliteRow> = query_builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|e| WrightError::DatabaseError(format!("failed to query find_owners_batch: {}", e)))?;

            for row in rows {
                use sqlx::Row;
                let path: String = row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                let name: String = row.try_get(1).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                result.insert(path, name);
            }
        }
        Ok(result)
    }

    pub async fn get_other_owners_batch(
        &self,
        current_part_id: i64,
        paths: &[&str],
    ) -> Result<HashMap<String, Vec<String>>> {
        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for chunk in paths.chunks(998) {
            let mut query_builder: QueryBuilder<Sqlite> = QueryBuilder::new(
                "SELECT f.path, p.name FROM parts p JOIN files f ON p.id = f.part_id WHERE p.id != "
            );
            query_builder.push_bind(current_part_id);
            query_builder.push(" AND f.path IN (");
            
            let mut separated = query_builder.separated(", ");
            for path in chunk {
                separated.push_bind(path);
            }
            separated.push_unseparated(")");

            let rows: Vec<sqlx::sqlite::SqliteRow> = query_builder
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|e| WrightError::DatabaseError(format!("failed to query get_other_owners_batch: {}", e)))?;

            for row in rows {
                use sqlx::Row;
                let path: String = row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                let name: String = row.try_get(1).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
                result.entry(path).or_default().push(name);
            }
        }
        Ok(result)
    }

    pub async fn get_file_ownership_conflicts(&self) -> Result<Vec<String>> {
        let rows = query(
            "SELECT path, GROUP_CONCAT(p.name, ', ') as owners
             FROM files f
             JOIN parts p ON f.part_id = p.id
             GROUP BY path
             HAVING COUNT(DISTINCT part_id) > 1")
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WrightError::DatabaseError(format!("failed to get ownership conflicts: {}", e)))?;

        let mut result = Vec::new();
        for row in rows {
            use sqlx::Row;
            let path: String = row.try_get(0).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            let owners: String = row.try_get(1).map_err(|e| WrightError::DatabaseError(e.to_string()))?;
            result.push(format!("Path '{}' is claimed by multiple parts: {}", path, owners));
        }
        Ok(result)
    }
}
