use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_PROVIDE_AFTER_HELP: &str = "\
Examples:
  wright provide glibc 2.41
  wright provide gcc 15.1.0";

#[derive(Args)]
#[command(
    long_about = "Mark a part as externally provided so dependency checks consider it satisfied.\n\nPass a name and version as arguments, pipe 'name version' lines, or use --file for bulk bootstrap. Remove them later with `wright remove`.",
    after_help = WRIGHT_PROVIDE_AFTER_HELP
)]
pub struct ProvideArgs {
    /// Part name (omit if piping or using --file)
    #[arg(value_name = "PART")]
    pub name: Option<String>,
    /// Part version (omit if piping or using --file)
    pub version: Option<String>,
    /// Read 'name version' pairs from a file (one per line)
    #[arg(long, value_name = "FILE")]
    pub file: Option<std::path::PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: ProvideArgs, ctx: &Context<'_>) -> Result<()> {
    let (_, _lock) = ctx.ensure_lock_and_part_store()?;
    let db = ctx.open_db().await?;
    crate::operations::provide::execute_provide(
        &db,
        args.name.as_deref(),
        args.version.as_deref(),
        args.file.as_deref(),
    )
    .await
}
