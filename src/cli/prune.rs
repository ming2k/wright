use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_PRUNE_AFTER_HELP: &str = "\
Examples:
  wright prune           # Show older archive versions that can be removed
  wright prune --apply   # Remove older archives, retaining the latest version";

#[derive(Args)]
#[command(
    long_about = "Remove older local archive versions while retaining the latest version of each part. The default is a dry run; pass --apply to actually delete.",
    after_help = WRIGHT_PRUNE_AFTER_HELP
)]
pub struct PruneArgs {
    /// Keep only the latest archive version for each part name (currently the only mode)
    #[arg(long)]
    pub latest: bool,

    /// Actually apply file deletions (default is dry-run)
    #[arg(long)]
    pub apply: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: PruneArgs, ctx: &Context<'_>) -> Result<()> {
    // --latest is accepted for forward compatibility with future prune modes;
    // latest-retention is currently the only behavior.
    let _ = args.latest;
    crate::operations::prune::execute_prune(args.apply, ctx.config).await
}
