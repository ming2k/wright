use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_PRUNE_AFTER_HELP: &str = "\
Examples:
  wright prune --latest          # Show older archive versions that can be removed
  wright prune --latest --apply  # Remove older archives, retaining the latest version";

#[derive(Args)]
#[command(
    long_about = "Remove older local archive versions while retaining the latest version of each part. The default is a dry run.",
    after_help = WRIGHT_PRUNE_AFTER_HELP
)]
pub struct PruneArgs {
    /// Keep only the latest archive version for each part name
    #[arg(long, required = true)]
    pub latest: bool,

    /// Actually apply file deletions (default is dry-run)
    #[arg(long)]
    pub apply: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: PruneArgs, ctx: &Context<'_>) -> Result<()> {
    debug_assert!(args.latest, "clap requires a prune mode");
    crate::operations::prune::execute_prune(args.apply, ctx.config).await
}
