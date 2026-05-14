use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

pub const LAUNCH_AFTER_HELP: &str = "\
Examples:
  wright launch --root /mnt/new --folio ./folios/core.toml
  wright launch --root /mnt/new --plans ./plans bash coreutils glibc
  wright launch --root /mnt/new --plans ./plans @core
  wright launch --root /mnt/new @base @maintenance @desktop

Launch fills a target root from a folio manifest or from explicit plan names.
When --folio is given, the manifest names the plans to resolve, forge, and
deploy.  When positional arguments are given, they are plan names or folio
names prefixed with '@'.  If --plans is omitted, the default plans directory
from wright.toml is used.  Re-running launch on the same root converges drift
rather than erroring.";

#[derive(Args, Debug)]
#[command(after_help = LAUNCH_AFTER_HELP)]
pub struct LaunchArgs {
    /// Path to a `folio.toml` file. Mutually exclusive with --plans.
    #[arg(long, value_name = "FILE", conflicts_with = "plans")]
    pub folio: Option<PathBuf>,

    /// Source path: take plans from this directory and apply them into --root.
    /// Positional arguments are plan names, or folio names prefixed with '@'.
    #[arg(long, value_name = "DIR", conflicts_with = "folio")]
    pub plans: Option<PathBuf>,

    /// Plan or folio names to launch when using --plans.
    /// Names starting with '@' are resolved as folios under the plans directory.
    #[arg(value_name = "TARGET")]
    pub plan_targets: Vec<String>,

    /// Print deploy order and config actions without writing anything.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Reforge and redeploy parts that already exist in the target root.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: LaunchArgs, ctx: &Context<'_>) -> Result<()> {
    let request = crate::operations::launch::LaunchRequest {
        folio: args.folio,
        plans: args.plans,
        plan_targets: args.plan_targets,
        dry_run: args.dry_run,
        force: args.force,
    };
    crate::operations::launch::execute_launch(
        request,
        ctx.config,
        &ctx.db_path,
        &ctx.root_dir,
        ctx.verbose,
        ctx.quiet,
    )
    .await
}
