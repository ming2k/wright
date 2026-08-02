use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_CLEAN_AFTER_HELP: &str = "\
Examples:
  wright clean                  # Clean build staging trees for all plans
  wright clean hello            # Clean build staging tree for plan 'hello'
  wright clean hello --parts    # Clean build workspace and built package archives for 'hello'
  wright clean --logs           # Also remove Wright command logs";

#[derive(Args)]
#[command(
    long_about = "Clean plan build workspaces, staging trees, intermediate objects, and logs.",
    after_help = WRIGHT_CLEAN_AFTER_HELP
)]
pub struct CleanArgs {
    /// Plan names to clean (defaults to all plans if omitted)
    #[arg(value_name = "PLAN")]
    pub plans: Vec<String>,

    /// Clean built package archives (.wright.tar.zst) as well
    #[arg(long, short)]
    pub parts: bool,

    /// Clean Wright command log files
    #[arg(long, short)]
    pub logs: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: CleanArgs, ctx: &Context<'_>) -> Result<()> {
    crate::operations::clean::execute_clean(&args.plans, args.parts, args.logs, ctx.config).await
}
