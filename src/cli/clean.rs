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
  wright clean --store            # Delete CAS store entries linked to no archive
  wright clean --sources          # Delete cached source files
  wright clean --ledger --keep-builds 5
  wright clean --logs -n          # Preview log cleanup with byte totals";

#[derive(Args)]
#[command(
    long_about = "Reclaim disk space across every location Wright owns (ADR-0043).\n\n\
                  By default only build workspaces are removed: all of them, or those \
                  of the named plans. Each flag adds one location with its own \
                  predicate; combine them freely. `--stale` is archive-retention \
                  mode: it removes only superseded (non-latest) archive versions of \
                  each (plan, output) pair.\n\n\
                  Every dry run reports reclaimed bytes. `wright storage` shows the same \
                  locations and the flag that reclaims each.",
    after_help = WRIGHT_CLEAN_AFTER_HELP
)]
pub struct CleanArgs {
    /// Plan names to clean (defaults to all plans if omitted)
    #[arg(value_name = "TARGET")]
    pub plans: Vec<String>,

    /// Also delete built part archives (.wright.tar.zst) sealed by the named
    /// plans (matched by archive plan metadata)
    #[arg(long)]
    pub archives: bool,

    /// Delete only superseded (non-latest) archive versions of each
    /// (plan, output) pair; skips workspace cleanup
    #[arg(long, conflicts_with = "archives")]
    pub stale: bool,

    /// Delete CAS store entries whose inode is not shared with any part
    /// archive (orphaned cache copies that free real space when unlinked)
    #[arg(long)]
    pub store: bool,

    /// Delete cached source files (re-fetched on the next build)
    #[arg(long)]
    pub sources: bool,

    /// With --sources, only delete sources older than this many days
    #[arg(long, value_name = "DAYS", requires = "sources")]
    pub older_than_days: Option<u64>,

    /// Rotate the audit ledger (per-plan build records and plan snapshots)
    #[arg(long)]
    pub ledger: bool,

    /// With --ledger, build records to keep per plan
    #[arg(long, value_name = "N", default_value_t = 10, requires = "ledger")]
    pub keep_builds: usize,

    /// With --ledger, plan snapshots to keep per plan
    #[arg(long, value_name = "N", default_value_t = 10, requires = "ledger")]
    pub keep_snapshots: usize,

    /// Also remove Wright command log files
    #[arg(long)]
    pub logs: bool,

    /// Preview what would be deleted, with byte totals, without removing anything
    #[arg(long, short = 'n')]
    pub dry_run: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: CleanArgs, ctx: &Context<'_>) -> Result<()> {
    crate::operations::clean::execute_clean(
        crate::operations::clean::CleanRequest {
            plans: &args.plans,
            archives: args.archives,
            stale: args.stale,
            logs: args.logs,
            store: args.store,
            sources: args.sources,
            older_than_days: args.older_than_days,
            ledger: args.ledger,
            keep_builds: args.keep_builds,
            keep_snapshots: args.keep_snapshots,
            dry_run: args.dry_run,
        },
        ctx.config,
    )
    .await
}
