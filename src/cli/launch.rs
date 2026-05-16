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
  wright launch --root /mnt/new --plans ./plans --folios ./folios @core
  wright launch --root /mnt/new @base @maintenance @desktop

Launch fills a target root from a folio manifest or from explicit plan
names.  --folio takes a single manifest path.  Positional arguments are
plan names, or folio names prefixed with '@' (resolved under --folios or
the configured folios_dir).  --plans and --folios are peer overrides.
Re-running on the same root converges drift rather than erroring.";

#[derive(Args, Debug)]
#[command(after_help = LAUNCH_AFTER_HELP)]
pub struct LaunchArgs {
    /// Path to a single folio manifest. Mutually exclusive with positional targets.
    #[arg(
        long,
        value_name = "FILE",
        conflicts_with_all = ["plans", "folios", "plan_targets"],
    )]
    pub folio: Option<PathBuf>,

    /// Override the plans search directory for this launch.
    #[arg(long, value_name = "DIR")]
    pub plans: Option<PathBuf>,

    /// Override the folios search directory for this launch.
    #[arg(long, value_name = "DIR")]
    pub folios: Option<PathBuf>,

    /// Plan names, or folio names prefixed with '@'.
    #[arg(value_name = "TARGET")]
    pub plan_targets: Vec<String>,

    /// Print deploy order and config actions without touching the target.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Reforge and redeploy parts that already exist in the target root.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Alternate root directory for file operations.
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: LaunchArgs, ctx: &Context<'_>) -> Result<()> {
    use crate::operations::launch::{LaunchRequest, LaunchSource, execute_launch};

    let source = match args.folio {
        Some(path) => LaunchSource::Folio(path),
        None => LaunchSource::Targets {
            plans_dir: args.plans,
            folios_dir: args.folios,
            targets: args.plan_targets,
        },
    };

    execute_launch(
        LaunchRequest {
            source,
            dry_run: args.dry_run,
            force: args.force,
        },
        ctx.config,
        &ctx.db_path,
        &ctx.root_dir,
        ctx.verbose,
        ctx.quiet,
    )
    .await
}
