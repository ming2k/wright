use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_PACKAGE_AFTER_HELP: &str = "\
Examples:
  wright package hello          # Seal built staging directory into .wright.tar.zst
  wright seal hello             # Alias for package";

#[derive(Args)]
#[command(
    name = "package",
    alias = "seal",
    long_about = "Seal built staging directories into .wright.tar.zst archives (Step 3 of Delivery pipeline).",
    after_help = WRIGHT_PACKAGE_AFTER_HELP
)]
pub struct PackageArgs {
    /// Plan names or plan directory paths to package
    #[arg(required = true, value_name = "PLAN")]
    pub plans: Vec<String>,

    /// Force re-slicing of output directories before packaging
    #[arg(long, short)]
    pub force: bool,

    /// Print generated archive paths to stdout
    #[arg(long, short)]
    pub print_parts: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: PackageArgs, ctx: &Context<'_>) -> Result<()> {
    crate::operations::package::execute_package(
        &args.plans,
        args.print_parts,
        args.force,
        ctx.config,
    )
    .await
}
