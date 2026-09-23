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
    #[value(name = "build")]
    Build,
    /// Follow all relationships (link + runtime + build).
    All,
}

#[cfg(with_handlers)]
pub(crate) fn map_domain(domain: DomainArg) -> crate::resolve::DepDomain {
    match domain {
        DomainArg::Link => crate::resolve::DepDomain::LINK,
        DomainArg::Runtime => crate::resolve::DepDomain::RUNTIME,
        DomainArg::Build => crate::resolve::DepDomain::BUILD,
        DomainArg::All => crate::resolve::DepDomain::ALL,
    }
}

#[cfg(with_handlers)]
pub(crate) fn map_match_policy(policy: MatchPolicyArg) -> crate::resolve::MatchPolicy {
    match policy {
        MatchPolicyArg::Missing => crate::resolve::MatchPolicy::Missing,
        MatchPolicyArg::Outdated => crate::resolve::MatchPolicy::Outdated,
        MatchPolicyArg::Installed => crate::resolve::MatchPolicy::Installed,
        MatchPolicyArg::All => crate::resolve::MatchPolicy::All,
    }
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
use crate::database::{InstalledDb, ReadOnlyDb};
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
    /// Open the installed-state database for mutation.
    ///
    /// Only System- and Local-class commands may call this: it acquires an
    /// exclusive process lock and runs pending migrations (ADR-0044).
    pub async fn open_db(&self) -> Result<InstalledDb> {
        InstalledDb::open(
            &self.db_path,
            Some(&crate::ledger::dir(self.config, Some(&self.db_path))),
        )
        .await
        .map_err(|e| WrightError::context("failed to open database", e))
    }

    /// Open the installed-state database read-only.
    ///
    /// This is the entry point for every Read-class command: no directory or
    /// database creation, no migrations, and no process lock — reads rely on
    /// WAL snapshot isolation and never block on a writer (ADR-0044,
    /// `[INV-PRIV-01]`).
    pub async fn open_read_only(&self) -> Result<ReadOnlyDb> {
        ReadOnlyDb::open_read_only(&self.db_path)
            .await
            .map_err(|e| WrightError::context("failed to open database", e))
    }

    pub fn ensure_lock_and_part_store(&self) -> Result<(LocalPartStore, ProcessLock)> {
        let lock = crate::util::lock::acquire_lock(
            &crate::util::lock::lock_dir_from_db(&self.db_path),
            crate::util::lock::LockIdentity::Command("wright"),
            crate::util::lock::LockMode::Exclusive,
        )
        .map_err(|e| WrightError::context("failed to start wright operation", e))?;
        let part_store = crate::resolve::setup_part_store(self.config)?;
        Ok((part_store, lock))
    }
}

/// Recover state left by a crashed System-class command.
///
/// This opens the database read-write (running migrations), performs delivery
/// recovery and mid-flight removal rollback, then drops the handle so the
/// command can reopen it. It is called *only* for System-class commands: a
/// Read- or Local-class command must never write, and recovery mutates the
/// database (ADR-0044, `[INV-PRIV-03]`).
#[cfg(with_handlers)]
pub(crate) async fn crash_recover(db_path: &Path, config: &GlobalConfig, root_dir: &Path) {
    let export_dir = crate::ledger::dir(config, Some(db_path));
    if let Ok(db) = InstalledDb::open(db_path, Some(&export_dir)).await {
        let _ = crate::delivery::recover_if_needed(&db).await;
        // Undo any removal the previous run left mid-flight. Runs after
        // delivery recovery so an APPLYING removal delivery has already been
        // settled; the registry then decides each journal's fate.
        let _ = crate::transaction::recover_transactions(root_dir, &db).await;
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
