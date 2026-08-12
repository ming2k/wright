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
  wright check llvm            # every deployed output of plan llvm
  wright check llvm:clang      # one output, absolute form
  wright check --deep
  wright check --integrity-only
  wright check --json";

#[derive(Args)]
#[command(
    long_about = "Run system health checks covering database integrity, file conflicts, \
                  shadowed files, and runtime dependency resolution.\n\n\
                  Targets follow the universal plan/output identifier: `plan` or `plan:*` \
                  checks every deployed output of a plan, `output` or `plan:output` checks \
                  a single output. A bare name matching both a plan and an output is \
                  rejected as ambiguous; use an absolute form instead.\n\n\
                  With --deep, walk each deployed part's ELF binaries and \
                  verify their DT_NEEDED entries against the deployed \
                  file ownership table. This catches forgotten declarations \
                  that the registry-level check would miss.\n\n\
                  With --files, verify that every deployed file recorded in \
                  the database still exists on disk.  Use this to detect \
                  partially-uninstalled parts or files deleted by external \
                  tools.\n\n\
                  With --json, print a machine-readable report to stdout: \
                  {\"scope\", \"mode\", \"issue_count\", \"issues\": [...]} \
                  where each issue carries a `check` tag identifying the \
                  check that produced it.\n\n\
                  Per ADR-0016 the registry is advisory: this command \
                  reports state, it does not change it. Exit code is 0 \
                  when everything resolves and 1 when any unsatisfied \
                  edge exists, so it is suitable for CI gates.",
    after_help = WRIGHT_CHECK_AFTER_HELP
)]
pub struct CheckArgs {
    /// Restrict the check to a plan or output target (`plan`, `plan:*`,
    /// `output`, or `plan:output`); omit for registry-level scope.
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,

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

    /// Emit a machine-readable JSON report instead of text
    #[arg(long)]
    pub json: bool,

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
        args.target.as_deref(),
        args.deep,
        args.integrity_only,
        args.check_files,
        args.json,
    )
    .await
}
