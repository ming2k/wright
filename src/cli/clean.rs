use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_CLEAN_AFTER_HELP: &str = "\
Examples:
  wright clean                    # Clean build workspaces for all plans
  wright clean hello              # Clean the build workspace for plan 'hello'
  wright clean hello --archives   # Also delete built part archives for 'hello'
  wright clean --stale            # Delete only superseded archive versions
  wright clean --logs             # Also remove Wright command logs
  wright clean -n                 # Preview what would be deleted";

#[derive(Args)]
#[command(
    long_about = "Reclaim disk space by deleting build workspaces, built part archives, and command logs.\n\nBy default only build workspaces are removed: all of them, or those of the named plans. `--stale` selects archive-retention mode instead: it removes only superseded (non-latest) archive versions of each (plan, output) pair and leaves workspaces alone.",
    after_help = WRIGHT_CLEAN_AFTER_HELP
)]
pub struct CleanArgs {
    /// Plan names to clean (defaults to all plans if omitted)
    #[arg(value_name = "TARGET")]
    pub plans: Vec<String>,

    /// Also delete built part archives (.wright.tar.zst) sealed by the named
    /// plans (matched by archive plan metadata)
    #[arg(long, alias = "parts")]
    pub archives: bool,

    /// Delete only superseded (non-latest) archive versions of each
    /// (plan, output) pair; skips workspace cleanup
    #[arg(long, conflicts_with = "archives")]
    pub stale: bool,

    /// Also remove Wright command log files
    #[arg(long)]
    pub logs: bool,

    /// Preview what would be deleted without removing anything
    #[arg(long, short = 'n')]
    pub dry_run: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: CleanArgs, ctx: &Context<'_>) -> Result<()> {
    crate::operations::clean::execute_clean(
        &args.plans,
        args.archives,
        args.stale,
        args.logs,
        args.dry_run,
        ctx.config,
    )
    .await
}
