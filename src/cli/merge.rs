use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_MERGE_AFTER_HELP: &str = "\
Examples:
  wright merge zlib
  wright merge ./plans/zlib
  wright merge --path ./zlib-1.3.1-1-x86_64.wright.tar.zst
  wright merge zlib --dry-run";

#[derive(Args)]
#[command(
    long_about = "Merge part archives into the target root.\n\nBy default, arguments are plan names or plan directories. Wright reads each plan manifest, derives the expected output archive names, and merges those archives from parts_dir into the system. Use --path to merge explicit archive paths instead. Runtime dependencies are checked for warnings and recorded in the database, but missing runtime dependencies do not block merging.",
    after_help = WRIGHT_MERGE_AFTER_HELP
)]
pub struct MergeArgs {
    /// Plan names/directories, or archive paths when using --path
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Force redeploy even if already deployed
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Skip runtime dependency warnings
    #[arg(long)]
    pub nodeps: bool,

    /// Treat arguments and stdin as explicit archive paths
    #[arg(long)]
    pub path: bool,

    /// Preview which archives would be deployed without making any changes
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: MergeArgs, ctx: &Context<'_>) -> Result<()> {
    let (part_store, _lock) = ctx.ensure_lock_and_part_store()?;
    crate::operations::merge::execute_merge(
        args.targets,
        args.force,
        args.nodeps,
        args.path,
        args.dry_run,
        ctx.config,
        &ctx.db_path,
        &ctx.root_dir,
        &part_store,
    )
    .await
}
