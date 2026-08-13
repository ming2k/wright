use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_LIST_AFTER_HELP: &str = "\
Examples:
  wright list
  wright list -l
  wright list --plan-only
  wright list --part-only
  wright list --filter leaf
  wright list --filter orphan
  wright list --filter provided
  wright list --json

By default deployed parts are listed as canonical target identifiers (plan:output, e.g. optics:flux).
Use --plan-only or --part-only to project output to a single namespace (suitable for piping),
-f/--filter to apply resource access graph (RAG) query filters (e.g. leaf, orphan, provided, deps(target)),
-l/--long to add origin, version, release, and architecture, or --json for machine-readable records.";

#[derive(Args)]
#[command(
    long_about = "List deployed targets, plans, and parts.\n\nUse graph filters to narrow output to leaf targets, provided external parts, or orphaned dependency deploys.",
    after_help = WRIGHT_LIST_AFTER_HELP
)]
pub struct ListArgs {
    /// Show origin, version, release, and architecture
    #[arg(long, short)]
    pub long: bool,
    /// Output only plan names (one per line)
    #[arg(long, conflicts_with = "part_only")]
    pub plan_only: bool,
    /// Output only bare part names (one per line)
    #[arg(long)]
    pub part_only: bool,
    /// Filter nodes by resource access graph / DAG relation query (e.g. leaf, orphan, provided)
    #[arg(long, short = 'f')]
    pub filter: Option<String>,
    /// Emit a machine-readable JSON array of records
    #[arg(long)]
    pub json: bool,
    /// Alternate root directory to query
    #[arg(long)]
    pub root: Option<std::path::PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: ListArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::list::execute_list(
        &db,
        args.long,
        args.filter.as_deref(),
        args.json,
        args.plan_only,
        args.part_only,
    )
    .await
}
