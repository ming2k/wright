use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

/// Deprecated alias for `wright clean --stale`, kept for one release.
/// `--apply` maps to execution; without it the command stays a dry run,
/// matching the historical prune default.
#[derive(Args)]
#[command(hide = true)]
pub struct PruneArgs {
    /// Accepted for backward compatibility; stale-retention is the only mode
    #[arg(long, hide = true)]
    pub latest: bool,

    /// Actually delete the selected archives (default is a dry run)
    #[arg(long)]
    pub apply: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: PruneArgs, ctx: &Context<'_>) -> Result<()> {
    let _ = args.latest;
    crate::cli_warn!("`wright prune` is deprecated; use `wright clean --stale` instead");
    crate::operations::clean::execute_clean(&[], false, true, false, !args.apply, ctx.config).await
}
