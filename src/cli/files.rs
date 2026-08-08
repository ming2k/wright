use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_FILES_AFTER_HELP: &str = "\
Examples:
  wright files zlib
  wright files zlib --json";

#[derive(Args)]
#[command(
    long_about = "List files recorded as owned by a deployed part.",
    after_help = WRIGHT_FILES_AFTER_HELP
)]
pub struct FilesArgs {
    /// Part name
    #[arg(value_name = "PART")]
    pub part: String,

    /// Emit machine-readable JSON instead of text
    #[arg(long)]
    pub json: bool,

    /// Alternate root directory to query
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: FilesArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::files::execute_files(&db, &args.part, args.json).await
}
