use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_LIST_AFTER_HELP: &str = "\
Examples:
  wright list
  wright list -l
  wright list --plans
  wright list --parts
  wright list --roots
  wright list --orphans
  wright list --provided
  wright list --json

By default deployed parts are grouped under the plan that produced them,
mirroring the plan/output targets accepted by commands like wright remove
and wright files. Use --plans or --parts for a flat one-per-line list of a
single namespace (suitable for piping), -l/--long to add origin, version,
release, and architecture, or --json for machine-readable records.";

#[derive(Args)]
#[command(
    long_about = "List deployed plans and parts.\n\nUse filters to narrow the output to root parts, provided external parts, or orphaned dependency deploys.",
    after_help = WRIGHT_LIST_AFTER_HELP
)]
pub struct ListArgs {
    /// Show origin, version, release, and architecture
    #[arg(long, short)]
    pub long: bool,
    /// List only plans (one per line), without their parts
    #[arg(long, conflicts_with = "parts")]
    pub plans: bool,
    /// List only parts (one per line), without plan grouping
    #[arg(long)]
    pub parts: bool,
    /// Show only top-level (root) parts with no deployed dependents
    #[arg(long)]
    pub roots: bool,
    /// Show only provided (externally provided) parts
    #[arg(long)]
    pub provided: bool,
    /// Show only orphan parts (auto-deployed deps no longer needed)
    #[arg(long, short)]
    pub orphans: bool,
    /// Emit a machine-readable JSON array of part (or plan, with --plans) records
    #[arg(long)]
    pub json: bool,
    /// Alternate root directory to query
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: ListArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::list::execute_list(
        &db,
        args.long,
        args.roots,
        args.provided,
        args.orphans,
        args.json,
        args.plans,
        args.parts,
    )
    .await
}
