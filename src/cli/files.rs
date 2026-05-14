use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_FILES_AFTER_HELP: &str = "\
Examples:
  wright files zlib";

#[derive(Args)]
#[command(
    long_about = "List files recorded as owned by a deployed part.",
    after_help = WRIGHT_FILES_AFTER_HELP
)]
pub struct FilesArgs {
    /// Part name
    #[arg(value_name = "PART")]
    pub part: String,
}

#[cfg(with_handlers)]
pub async fn run(args: FilesArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::files::execute_files(&db, &args.part).await
}
