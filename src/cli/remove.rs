use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_REMOVE_AFTER_HELP: &str = "\
Examples:
  wright remove zlib
  wright remove zlib --recursive
  wright remove zlib --cascade";

#[derive(Args)]
#[command(
    long_about = "Remove deployed parts by name.\n\nBy default, removal is blocked when another deployed part depends on the target. Use `--recursive` to remove dependents too, or `--force` to bypass safety checks.",
    after_help = WRIGHT_REMOVE_AFTER_HELP
)]
pub struct RemoveArgs {
    /// Part names to remove
    #[arg(required = true, value_name = "PART")]
    pub parts: Vec<String>,

    /// Force removal even if other parts depend on this one
    #[arg(long)]
    pub force: bool,

    /// Recursively remove all parts that depend on the target
    #[arg(long, short)]
    pub recursive: bool,

    /// Also remove orphan dependencies (auto-deployed deps no longer needed)
    #[arg(long, short = 'c')]
    pub cascade: bool,

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
        &ctx.root_dir,
    )
    .await
}
