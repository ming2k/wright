use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_FILES_AFTER_HELP: &str = "\
Examples:
  wright files zlib
  wright files llvm            # every deployed output of plan llvm
  wright files llvm:clang      # one output, absolute form
  wright files zlib --json";

#[derive(Args)]
#[command(
    long_about = "List files recorded as owned by deployed parts.\n\nTargets follow the universal plan/output identifier: `plan` or `plan:*` lists every deployed output of a plan, `output` or `plan:output` lists a single output. A bare name matching both a plan and an output is rejected as ambiguous; use an absolute form instead.",
    after_help = WRIGHT_FILES_AFTER_HELP
)]
pub struct FilesArgs {
    /// Plan or output target (`plan`, `plan:*`, `output`, or `plan:output`)
    #[arg(value_name = "TARGET")]
    pub target: String,

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
    crate::operations::files::execute_files(&db, &args.target, args.json).await
}
