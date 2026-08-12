use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_HISTORY_AFTER_HELP: &str = "\
Examples:
  wright history
  wright history zlib
  wright history llvm            # every deployed output of plan llvm
  wright history llvm:clang      # one output, absolute form
  wright history --json";

#[derive(Args)]
#[command(
    long_about = "Show part transaction history.\n\nPass a target to limit the history, or omit it to show all recorded transactions. Targets follow the universal plan/output identifier: `plan` or `plan:*` shows the merged history of every deployed output of a plan, `output` or `plan:output` shows a single output. A bare name matching both a plan and an output is rejected as ambiguous; use an absolute form instead.",
    after_help = WRIGHT_HISTORY_AFTER_HELP
)]
pub struct HistoryArgs {
    /// Plan or output target (`plan`, `plan:*`, `output`, or `plan:output`);
    /// omit to show all history
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,

    /// Emit machine-readable JSON instead of text
    #[arg(long)]
    pub json: bool,

    /// Alternate root directory to query
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: HistoryArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::history::execute_history(&db, args.target.as_deref(), args.json).await
}
