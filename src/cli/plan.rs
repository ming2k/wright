use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_PLAN_AFTER_HELP: &str = "\
Examples:
  wright plan zlib
  wright plan zlib > plan.toml   # recover the exact sealed source
  wright plan zlib --json";

#[derive(Args)]
#[command(
    long_about = "Print the plan source recorded when the plan's parts were sealed.\n\nThe snapshot is the exact plan.toml that produced the deployed parts, recovered from the local ledger even if the plan file has since been edited or deleted. Parts sealed before plan-source snapshots (ADR-0033) have none; rebuild and re-deploy to record one.",
    after_help = WRIGHT_PLAN_AFTER_HELP
)]
pub struct PlanArgs {
    /// Plan whose recorded source to print
    #[arg(value_name = "TARGET")]
    pub plan: String,

    /// Emit machine-readable JSON instead of raw plan source
    #[arg(long)]
    pub json: bool,

    /// Alternate root directory to query
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: PlanArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    let ledger_dir = crate::ledger::dir(ctx.config, Some(&ctx.db_path));
    crate::operations::plan::execute_plan(&db, &ledger_dir, &args.plan, args.json).await
}
