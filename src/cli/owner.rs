use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_OWNER_AFTER_HELP: &str = "\
Examples:
  wright owner /usr/bin/zlib-flate
  wright owner /usr/lib/libz.so.1 /etc/hostname";

#[derive(Args)]
#[command(
    long_about = "Show which deployed part owns each given file path. Relative paths \
                  are resolved against the current directory; symlinks are followed \
                  when the path exists.",
    after_help = WRIGHT_OWNER_AFTER_HELP
)]
pub struct OwnerArgs {
    /// File path(s) to look up
    #[arg(value_name = "FILE", required = true)]
    pub paths: Vec<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: OwnerArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::owner::execute_owner(&db, &args.paths).await
}
