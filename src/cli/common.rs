use clap::ValueEnum;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum MatchPolicyArg {
    /// Include plans that are not currently installed.
    Missing,
    /// Include plans whose version/release differs from the installed one.
    Outdated,
    /// Include plans that are already installed and match the plan definition.
    Installed,
    /// Include all plans.
    All,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DomainArg {
    /// Follow only ABI-sensitive link relationships.
    Link,
    /// Follow only runtime relationships.
    Runtime,
    /// Follow only build-time relationships.
    Forge,
    /// Follow all relationships (link + runtime + build).
    All,
}

// The items below reference crate::operations / util / resolve / delivery and
// are only visible when the main crate is compiled. build.rs `#[path]`-includes
// this file but does NOT see the `with_handlers` cfg (only the main crate
// compile does, via build.rs emitting `cargo::rustc-cfg=with_handlers`).
#[cfg(with_handlers)]
use std::path::{Path, PathBuf};

#[cfg(with_handlers)]
use crate::config::GlobalConfig;
#[cfg(with_handlers)]
use crate::database::InstalledDb;
#[cfg(with_handlers)]
use crate::error::{Result, WrightError};
#[cfg(with_handlers)]
use crate::part::store::LocalPartStore;
#[cfg(with_handlers)]
use crate::util::lock::ProcessLock;

/// Runtime context built once per invocation and passed to every command handler.
#[cfg(with_handlers)]
pub struct Context<'a> {
    pub config: &'a GlobalConfig,
    pub db_path: PathBuf,
    pub root_dir: PathBuf,
    pub verbose: u8,
    pub quiet: bool,
}

#[cfg(with_handlers)]
impl<'a> Context<'a> {
    pub async fn open_db(&self) -> Result<InstalledDb> {
        InstalledDb::open(&self.db_path).await.map_err(|e| {
            WrightError::DatabaseError(format!("failed to open database: {}", e))
        })
    }

    pub fn ensure_lock_and_part_store(&self) -> Result<(LocalPartStore, ProcessLock)> {
        let lock = crate::util::lock::acquire_lock(
            &crate::util::lock::lock_dir_from_db(&self.db_path),
            crate::util::lock::LockIdentity::Command("wright"),
            crate::util::lock::LockMode::Exclusive,
        )
        .map_err(|e| {
            WrightError::LockError(format!("failed to start wright operation: {}", e))
        })?;
        let part_store = crate::resolve::setup_part_store(self.config)?;
        Ok((part_store, lock))
    }
}

#[cfg(with_handlers)]
pub(crate) async fn crash_recover(db_path: &Path) {
    if let Ok(db) = InstalledDb::open(db_path).await {
        let _ = crate::delivery::recover_if_needed(&db).await;
    }
}

#[cfg(with_handlers)]
pub(crate) fn resolve_db(
    root: Option<&Path>,
    top_db: Option<PathBuf>,
    config: &GlobalConfig,
) -> PathBuf {
    top_db.unwrap_or_else(|| {
        if let Some(r) = root
            && r != Path::new("/")
        {
            return r.join("var/lib/wright/wright.db");
        }
        config.general.db_path.clone()
    })
}
