//! Part removal: journaled filesystem teardown plus a single registry commit.
//!
//! A removal batch is the unit of atomicity. Every part's filesystem work runs
//! under one [`FsTransaction`]; the registry is flipped for the whole batch in
//! one SQL transaction at the end (`InstalledDb::commit_removal_batch`). If any
//! part fails before that commit, the transaction restores every file and no
//! history row is left `pending`.
//!
//! Ownership is per exact path. A path shared with another deployed part is
//! left untouched; a directory is removed only when empty, so content Wright
//! never registered is never carried away.

use std::collections::HashSet;

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::error::{Result, WrightError};
use wright_registry::database::{
    FileType, HistoryAction, HistoryStatus, InstalledDb, SessionContext,
};

use super::fs_tx::{FsIntent, FsTransaction};
use super::get_hook;
use super::hooks::{log_running_hook, run_deploy_script};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;

/// A part the transaction is removing, used to build the journal header so
/// recovery can resolve the batch against the registry.
#[derive(Debug, Clone)]
pub struct PartRef {
    pub name: String,
    pub hash: Option<String>,
}

/// A removal in progress across one or more parts.
///
/// Drives filesystem teardown for each part, then commits the registry for the
/// whole batch at once. Construct with [`RemovalBatch::begin`]; drive with
/// [`RemovalBatch::remove_one`]; finish with [`RemovalBatch::commit`].
pub struct RemovalBatch<'a> {
    db: &'a InstalledDb,
    root: PathBuf,
    session: SessionContext,
    fs: FsTransaction,
    /// Parts whose filesystem teardown succeeded, in journal order.
    staged: Vec<String>,
    /// Config paths left on disk per staged part.
    residue: Vec<(String, Vec<String>)>,
}

impl<'a> RemovalBatch<'a> {
    /// Begin a removal batch covering `parts`.
    pub async fn begin(
        db: &'a InstalledDb,
        root: &Path,
        session: SessionContext,
        parts: &[PartRef],
    ) -> Result<Self> {
        let intents: Vec<FsIntent> = parts
            .iter()
            .map(|p| FsIntent::remove(&p.name, p.hash.as_deref()))
            .collect();
        let fs = FsTransaction::begin(root, &session.id, &intents)?;
        Ok(Self {
            db,
            root: root.to_path_buf(),
            session,
            fs,
            staged: Vec::new(),
            residue: Vec::new(),
        })
    }

    /// Tear down one part: hooks, filesystem, diversions. The registry is not
    /// touched — that happens in [`Self::commit`].
    ///
    /// `ignored_dependents` names dependents that are themselves being removed
    /// in this same batch, so they do not block the target.
    pub async fn remove_one(
        &mut self,
        name: &str,
        force: bool,
        ignored_dependents: &HashSet<String>,
    ) -> Result<()> {
        let part = self
            .db
            .get_part(name)
            .await?
            .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;

        let plan = self
            .db
            .get_plan_by_id(part.plan_id)
            .await?
            .ok_or_else(|| WrightError::PartNotFound(format!("plan for {}", name)))?;

        let mut dependents = self.db.get_dependents(name).await?;
        if !ignored_dependents.is_empty() {
            dependents.retain(|dep| !ignored_dependents.contains(dep));
        }
        if !dependents.is_empty() && !force {
            return Err(WrightError::DependencyError(format!(
                "cannot remove '{}': required by {}",
                name,
                dependents.join(", ")
            )));
        }
        if !dependents.is_empty() {
            warn!(
                event = "remove.forced",
                plan_name = name,
                dependents = dependents.join(", "),
                "Forcing removal of part with dependents"
            );
        }

        // Pending history row, settled at commit or rolled back on abort.
        self.db
            .record_history(
                &self.session.id,
                &self.session.command,
                name,
                HistoryAction::Remove,
                Some(&plan.version),
                None,
                part.part_hash.as_deref(),
                None,
                HistoryStatus::Pending,
                None,
            )
            .await?;

        if let Some(script) = part
            .deploy_scripts
            .as_deref()
            .and_then(|content| get_hook(content, "pre_remove"))
        {
            log_running_hook(name, "pre_remove");
            if let Err(e) = run_deploy_script(&script, &self.root, name, "pre_remove").await {
                warn!(
                    event = "remove.hook_failed",
                    plan_name = name,
                    hook = "pre_remove",
                    error = %e,
                    "Hook failed, continuing removal"
                );
            }
        }

        let preserved = self.teardown_files(part.id).await?;

        self.restore_diversions(part.id).await;

        if let Some(script) = part
            .deploy_scripts
            .as_deref()
            .and_then(|content| get_hook(content, "post_remove"))
        {
            log_running_hook(name, "post_remove");
            if let Err(e) = run_deploy_script(&script, &self.root, name, "post_remove").await {
                warn!(
                    event = "remove.hook_failed",
                    plan_name = name,
                    hook = "post_remove",
                    error = %e,
                    "Hook failed, continuing removal"
                );
            }
        }

        self.staged.push(name.to_string());
        self.residue.push((name.to_string(), preserved));
        Ok(())
    }

    /// Remove every file the part owns that no other part also owns, skipping
    /// config files, and journal each mutation. Returns the config paths left
    /// on disk.
    ///
    /// Two passes: files and symlinks first, then owned directories
    /// deepest-first. A directory must only be removed once its own contents
    /// are gone, and `get_files` orders paths ascending, so iterating the
    /// directory entries in reverse removes children before parents.
    async fn teardown_files(&mut self, part_id: i64) -> Result<Vec<String>> {
        let files = self.db.get_files(part_id).await?;
        let file_paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let other_owners = self.db.get_other_owners_batch(part_id, &file_paths).await?;
        let owns_only = |path: &str| {
            !other_owners
                .get(path)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
        };

        let mut preserved = Vec::new();

        // Pass 1: files and symlinks.
        for file in &files {
            if file.is_config {
                info!(
                    event = "remove.config_preserved",
                    path = file.path,
                    "Preserving config file"
                );
                preserved.push(file.path.clone());
                continue;
            }

            if !owns_only(&file.path) {
                debug!(
                    event = "remove.skip_shared_path",
                    path = file.path,
                    "Path is also owned by another part, skipping"
                );
                continue;
            }

            match file.file_type {
                // Symlinks are backed up and restored as symlinks by the
                // same-inode move.
                FileType::File | FileType::Symlink => {
                    self.fs.back_up_entry(&file.path, file.file_type).await?;
                }
                FileType::Directory => {}
            }
        }

        // Pass 2: directories, deepest first (descending path order).
        let mut dirs: Vec<&str> = files
            .iter()
            .filter(|f| f.file_type == FileType::Directory && !f.is_config)
            .map(|f| f.path.as_str())
            .collect();
        dirs.sort_unstable();
        for path in dirs.into_iter().rev() {
            if !owns_only(path) {
                continue;
            }
            let full = self.root.join(path.trim_start_matches('/'));
            self.fs.remove_empty_dir(&full).await?;
        }

        Ok(preserved)
    }

    /// Restore any files this part diverted away from other owners.
    async fn restore_diversions(&mut self, part_id: i64) {
        let diversions = match self.db.get_all_diverted_files(part_id).await {
            Ok(d) => d,
            Err(_) => return,
        };
        for (original, diverted) in diversions {
            let full_original = self.root.join(original.trim_start_matches('/'));
            let full_diverted = self.root.join(diverted.trim_start_matches('/'));
            if tokio::fs::symlink_metadata(&full_diverted).await.is_err() {
                continue;
            }
            info!(
                event = "remove.diversion_restored",
                path = original,
                "Restoring diverted file"
            );
            if let Err(e) = self.fs.move_aside(&full_diverted, &full_original).await {
                warn!(
                    event = "remove.diversion_restore_failed",
                    path = original,
                    error = %e,
                    "Failed to restore diverted file"
                );
            }
        }
    }

    /// Commit the batch: delete every staged part's registry rows and settle
    /// their history in one SQL transaction, then discard the backup store.
    /// Returns the parts actually removed.
    pub async fn commit(mut self) -> Result<Vec<String>> {
        let removed = self.db.commit_removal_batch(&self.staged).await?;

        for (name, paths) in &self.residue {
            if removed.contains(name) {
                let _ = self
                    .db
                    .record_removal_residue(&self.session.id, name, paths)
                    .await;
            }
        }

        self.fs.commit();
        Ok(removed)
    }

    /// Abort the batch: restore every journaled file and settle each pending
    /// history row as rolled back.
    pub async fn rollback(mut self) -> Result<()> {
        self.fs.rollback_blocking();
        let _ = self.db.rollback_history_session(&self.session.id).await;
        Ok(())
    }
}

/// Remove a single deployed part.
///
/// Used directly by `wright remove` for a single target and by the deploy path
/// when a `replaces` archive supersedes an installed part. The part's
/// dependents block removal unless `force` is set.
pub async fn remove_part(
    db: &InstalledDb,
    name: &str,
    root_dir: &Path,
    force: bool,
    session: SessionContext,
) -> Result<()> {
    let part = db
        .get_part(name)
        .await?
        .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;
    let refs = vec![PartRef {
        name: name.to_string(),
        hash: part.part_hash.clone(),
    }];

    let mut batch = RemovalBatch::begin(db, root_dir, session, &refs).await?;
    if let Err(e) = batch.remove_one(name, force, &HashSet::new()).await {
        batch.rollback().await?;
        return Err(e);
    }
    batch.commit().await?;
    info!(event = "remove.completed", plan_name = name, "Removed");
    Ok(())
}

pub async fn order_removal_batch(db: &InstalledDb, targets: &[String]) -> Result<Vec<String>> {
    let target_set: HashSet<String> = targets.iter().cloned().collect();
    let mut ordered = Vec::new();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();

    for name in targets {
        visit_removal_target(
            db,
            name,
            &target_set,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )
        .await?;
    }

    Ok(ordered)
}

fn visit_removal_target<'a>(
    db: &'a InstalledDb,
    name: &'a str,
    target_set: &'a HashSet<String>,
    visiting: &'a mut HashSet<String>,
    visited: &'a mut HashSet<String>,
    ordered: &'a mut Vec<String>,
) -> BoxFuture<'a, Result<()>> {
    async move {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_string()) {
            return Ok(());
        }

        let _ = db
            .get_part(name)
            .await?
            .ok_or_else(|| WrightError::PartNotFound(name.to_string()))?;
        let dependents = db.get_dependents(name).await?;
        let mut next: Vec<String> = dependents
            .into_iter()
            .filter(|dep_name| target_set.contains(dep_name))
            .collect();
        next.sort();
        next.dedup();

        for dep_name in next {
            visit_removal_target(db, &dep_name, target_set, visiting, visited, ordered).await?;
        }

        visiting.remove(name);
        visited.insert(name.to_string());
        ordered.push(name.to_string());
        Ok(())
    }
    .boxed()
}

pub async fn cascade_remove_list(db: &InstalledDb, name: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(name.to_string());
    cascade_collect(db, name, &mut visited, &mut result).await?;
    Ok(result)
}

fn cascade_collect<'a>(
    db: &'a InstalledDb,
    name: &'a str,
    visited: &'a mut HashSet<String>,
    result: &'a mut Vec<String>,
) -> BoxFuture<'a, Result<()>> {
    async move {
        let orphans = db.get_orphan_dependencies(name).await?;
        for orphan in orphans {
            if visited.contains(&orphan) {
                continue;
            }
            visited.insert(orphan.clone());
            cascade_collect(db, &orphan, visited, result).await?;
            result.push(orphan);
        }
        Ok(())
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wright_registry::database::{NewPart, NewPlan};

    async fn test_db() -> InstalledDb {
        InstalledDb::open_in_memory().await.unwrap()
    }

    async fn add_plan(db: &InstalledDb, name: &str, outputs: &[&str]) -> i64 {
        let plan_id = db
            .insert_plan(NewPlan {
                name,
                ..Default::default()
            })
            .await
            .unwrap();
        for output in outputs {
            db.insert_part(NewPart {
                name: output,
                plan_id,
                ..Default::default()
            })
            .await
            .unwrap();
        }
        plan_id
    }

    async fn link(db: &InstalledDb, from: &str, to: &str) {
        let part = db.get_part(from).await.unwrap().unwrap();
        db.insert_dependencies(
            part.id,
            &[wright_registry::database::Dependency {
                name: to.to_string(),
                version_constraint: None,
            }],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn order_removal_batch_puts_dependents_first() {
        let db = test_db().await;
        add_plan(&db, "lib", &["lib"]).await;
        add_plan(&db, "app", &["app"]).await;
        link(&db, "app", "lib").await;

        let order = order_removal_batch(&db, &["app".to_string(), "lib".to_string()])
            .await
            .unwrap();
        let app_at = order.iter().position(|n| n == "app").unwrap();
        let lib_at = order.iter().position(|n| n == "lib").unwrap();
        assert!(app_at < lib_at, "dependent must be removed before its dep");
    }

    #[tokio::test]
    async fn cascade_lists_orphan_dependencies() {
        let db = test_db().await;
        // `cascade` only collects *auto-deployed* dependencies (origin =
        // dependency), so lib must be inserted with that origin.
        let lib_plan = db
            .insert_plan(NewPlan {
                name: "lib",
                ..Default::default()
            })
            .await
            .unwrap();
        db.insert_part(NewPart {
            name: "lib",
            plan_id: lib_plan,
            origin: wright_registry::database::Origin::Dependency,
            ..Default::default()
        })
        .await
        .unwrap();
        add_plan(&db, "app", &["app"]).await;
        link(&db, "app", "lib").await;

        let list = cascade_remove_list(&db, "app").await.unwrap();
        assert_eq!(list, vec!["lib".to_string()]);
    }
}
