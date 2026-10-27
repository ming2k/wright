use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::{Result, WrightError};

const WRIGHT_UPGRADE_AFTER_HELP: &str = "\
Examples:
  wright upgrade zlib
  wright upgrade zlib --deps-depth (or --deep)
  wright upgrade zlib --rdeps-depth=1 (or --impact=1)
  wright upgrade zlib --deep --impact=1
  wright upgrade all
  wright upgrade all --dry-run
  wright upgrade all --fresh
  wright upgrade all --force
  wright upgrade all --no-cache
  wright upgrade all --no-cache --dry-run";

#[derive(Args)]
#[command(
    long_about = "Upgrade plans to the latest version.\n\nWhen given plan names, `wright` checks if the plan has a newer version than what is deployed, then resolves, forges, seals, and deploys it along with any installed parts that link-depend on it (to ensure ABI consistency). Use `--deps-depth` (or `--deep`) to recursively upgrade its entire forward dependency chain bottom-up. Use `--rdeps-depth=1` (or `--impact=1`) to restrict reverse dependency rebuilds to 1-hop direct consumers. Use `all` to check every installed plan for updates.\n\nFor archive-based upgrades, use `wright merge --force`.",
    after_help = WRIGHT_UPGRADE_AFTER_HELP
)]
pub struct UpgradeArgs {
    /// Plan names to upgrade, or `all` to upgrade all outdated plans
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Force rebuild and redeploy even if the plan version matches
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Wipe the forge workspace (source and working trees) before building,
    /// so plans that need an upgrade are forged from scratch. Unlike
    /// `--force`, this does not redeploy plans that are already up to date.
    /// Composable with `--force`.
    #[arg(long, short = 'c')]
    pub fresh: bool,

    /// Cold rebuild: transactionally purge the source cache and the CAS store
    /// (verified pinned http inputs are kept), then rebuild every selected plan
    /// from freshly fetched sources, bypassing all derived reuse. `all` selects
    /// every installed plan. Stronger than `--fresh`, which clears only the
    /// per-plan forge workspace. Combine with `--dry-run` to preview what would
    /// be purged.
    #[arg(long)]
    pub no_cache: bool,

    /// Preview what would be rebuilt and deployed without making any changes
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Maximum depth for forward dependency expansion (`0` means unlimited, upgrading entire bottom-up chain).
    /// Bare `--deps-depth` or `--deep` defaults to `0`.
    #[arg(
        long,
        value_name = "N",
        num_args = 0..=1,
        default_missing_value = "0",
        alias = "deep",
        alias = "upgrade-deps"
    )]
    pub deps_depth: Option<usize>,

    /// Maximum depth for reverse dependency expansion (`0` means unlimited).
    /// Sets the impact blast radius (e.g. `1` rechecks direct consumers only).
    /// Bare `--rdeps-depth` or `--impact` defaults to `1`.
    #[arg(
        long,
        value_name = "N",
        num_args = 0..=1,
        default_missing_value = "1",
        alias = "impact"
    )]
    pub rdeps_depth: Option<usize>,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Do not inhibit reverse-dependency rebuilds even if physical ABI probe proves backward compatibility
    #[arg(long)]
    pub no_inhibit_rebuild: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: UpgradeArgs, ctx: &Context<'_>) -> Result<()> {
    let (part_store, _lock) = ctx.ensure_lock_and_part_store()?;

    // Cold upgrades purge shared caches, so they must also exclude ordinary
    // builds, which hold only the `build` lock. Lock order is `wright` then
    // `build` (ADR-0057).
    let _build_lock = if args.no_cache {
        Some(
            crate::util::lock::acquire_lock(
                &crate::util::lock::lock_dir_from_db(&ctx.db_path),
                crate::util::lock::LockIdentity::Command("build"),
                crate::util::lock::LockMode::Exclusive,
            )
            .map_err(|e| WrightError::context("failed to acquire cold-build cache lock", e))?,
        )
    } else {
        None
    };

    crate::operations::upgrade::execute_upgrade(
        args.targets,
        args.force,
        args.fresh,
        args.no_cache,
        args.dry_run,
        args.deps_depth,
        args.rdeps_depth,
        args.no_inhibit_rebuild,
        ctx.config,
        &ctx.db_path,
        &ctx.root_dir,
        ctx.verbose,
        ctx.quiet,
        &part_store,
    )
    .await
}
