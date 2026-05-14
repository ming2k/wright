use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_UPGRADE_AFTER_HELP: &str = "\
Examples:
  wright upgrade zlib
  wright upgrade zlib openssl
  wright upgrade all
  wright upgrade all --force";

#[derive(Args)]
#[command(
    long_about = "Upgrade plans to the latest version.\n\nWhen given plan names, `wright` checks if the plan has a newer version than what is deployed, then resolves, forges, seals, and deploys it along with any installed parts that link-depend on it (to ensure ABI consistency). Use `all` to check every installed plan for updates.\n\nFor archive-based upgrades, use `wright merge --force`.",
    after_help = WRIGHT_UPGRADE_AFTER_HELP
)]
pub struct UpgradeArgs {
    /// Plan names to upgrade, or `all` to upgrade all outdated plans
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Force rebuild and redeploy even if the plan version matches
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Maximum depth for reverse dependency expansion. `0` means unlimited.
    #[arg(long)]
    pub depth: Option<usize>,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: UpgradeArgs, ctx: &Context<'_>) -> Result<()> {
    let (part_store, _lock) = ctx.ensure_lock_and_part_store()?;
    crate::operations::upgrade::execute_upgrade(
        args.targets,
        args.force,
        args.depth,
        ctx.config,
        &ctx.db_path,
        &ctx.root_dir,
        ctx.verbose,
        ctx.quiet,
        &part_store,
    )
    .await
}
