use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_REMOVE_AFTER_HELP: &str = "\
Examples:
  wright remove zlib
  wright remove llvm              # every deployed output of plan llvm
  wright remove llvm:clang        # one output, absolute form
  wright remove zlib --recursive
  wright remove zlib --cascade
  wright remove zlib --dry-run";

#[derive(Args)]
#[command(
    long_about = "Remove deployed parts.\n\nTargets follow the universal plan/output identifier: `plan` or `plan:*` removes every deployed output of a plan, `output` or `plan:output` removes a single output. A bare name matching both a plan and an output is rejected as ambiguous; use an absolute form instead.\n\nBy default, removal is blocked when another deployed part depends on the target. Use `--recursive` to remove dependents too, or `--force` to bypass safety checks.",
    after_help = WRIGHT_REMOVE_AFTER_HELP
)]
pub struct RemoveArgs {
    /// Plan or output targets to remove (`plan`, `plan:*`, `output`, or `plan:output`)
    #[arg(required = true, value_name = "TARGET")]
    pub parts: Vec<String>,

    /// Force removal even if other parts depend on this one
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Recursively remove all parts that depend on the target
    #[arg(long)]
    pub recursive: bool,

    /// Also remove orphan dependencies (auto-deployed deps no longer needed)
    #[arg(long)]
    pub cascade: bool,

    /// Preview which parts would be removed without making any changes
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: RemoveArgs, ctx: &Context<'_>) -> Result<()> {
    let (_, _lock) = ctx.ensure_lock_and_part_store()?;
    let db = ctx.open_db().await?;
    let parts_refs: Vec<&str> = args.parts.iter().map(|s| s.as_str()).collect();
    crate::operations::remove::execute_remove(
        &db,
        &parts_refs,
        args.force,
        args.recursive,
        args.cascade,
        args.dry_run,
        &ctx.root_dir,
    )
    .await
}
