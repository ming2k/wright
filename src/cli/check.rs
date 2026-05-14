use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_CHECK_AFTER_HELP: &str = "\
Examples:
  wright check
  wright check zlib
  wright check --deep
  wright check --integrity-only";

#[derive(Args)]
#[command(
    long_about = "Run system health checks covering database integrity, file conflicts, \
                  shadowed files, and runtime dependency resolution.\n\n\
                  With --deep, walk each deployed part's ELF binaries and \
                  verify their DT_NEEDED entries against the deployed \
                  file ownership table. This catches forgotten declarations \
                  that the registry-level check would miss.\n\n\
                  With --files, verify that every deployed file recorded in \
                  the database still exists on disk.  Use this to detect \
                  partially-uninstalled parts or files deleted by external \
                  tools.\n\n\
                  Per ADR-0016 the registry is advisory: this command \
                  reports state, it does not change it. Exit code is 0 \
                  when everything resolves and 1 when any unsatisfied \
                  edge exists, so it is suitable for CI gates.",
    after_help = WRIGHT_CHECK_AFTER_HELP
)]
pub struct CheckArgs {
    /// Restrict the check to a single deployed part (registry-level
    /// scope is unchanged when omitted).
    #[arg(value_name = "PART")]
    pub part: Option<String>,

    /// Walk ELF DT_NEEDED entries for each deployed binary and verify
    /// their providing parts via the files table. Reads disk; slower
    /// than the registry-level scan.
    #[arg(long)]
    pub deep: bool,

    /// Only run integrity checks (database, file conflicts, shadows)
    #[arg(long, conflicts_with_all = ["deep", "check_files"])]
    pub integrity_only: bool,

    /// Verify every deployed file exists on disk
    #[arg(long = "files")]
    pub check_files: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: CheckArgs, ctx: &Context<'_>) -> Result<()> {
    let db = ctx.open_db().await?;
    crate::operations::check::execute_check(
        &db,
        &ctx.root_dir,
        args.part.as_deref(),
        args.deep,
        args.integrity_only,
        args.check_files,
    )
    .await
}
