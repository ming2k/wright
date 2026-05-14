use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_HISTORY_AFTER_HELP: &str = "\
Examples:
  wright history
  wright history zlib";

#[derive(Args)]
#[command(
    long_about = "Show part transaction history.\n\nPass a part name to limit the history to one part, or omit it to show all recorded transactions.",
    after_help = WRIGHT_HISTORY_AFTER_HELP
)]
pub struct HistoryArgs {
    /// Part name; omit to show all history
    #[arg(value_name = "PART")]
    pub part: Option<String>,
}

#[cfg(with_handlers)]
pub async fn run(args: HistoryArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::history::execute_history(&db, args.part.as_deref()).await
}
