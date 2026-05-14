use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_DOCTOR_AFTER_HELP: &str = "\
Examples:
  wright doctor";

#[derive(Args)]
#[command(
    long_about = "Run comprehensive system health checks.\n\n\
                  This command performs all checks from `check --deep` and \
                  additionally verifies the dependency closure of archives \
                  in parts_dir. Use it after batch deployments to detect \
                  missing providers and stale dependencies across the entire \
                  archive collection.",
    after_help = WRIGHT_DOCTOR_AFTER_HELP
)]
pub struct DoctorArgs {
    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(_args: DoctorArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::doctor::execute_doctor(&db, &ctx.root_dir, ctx.config).await
}
