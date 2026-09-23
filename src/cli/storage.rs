use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

const WRIGHT_STORAGE_AFTER_HELP: &str = "\
Examples:
  wright storage
  wright storage --json

Every row names the command that reclaims it. Measure before you delete: a
location without a row here has no deletion flag.";

#[derive(Args)]
#[command(
    long_about = "Report disk usage for every location Wright owns and the command \
                  that reclaims each one (ADR-0043).\n\n\
                  Covers build workspaces, part archives, the CAS store, the source \
                  cache, command logs, the audit ledger, and the database file plus \
                  its WAL sidecar, plus the deployed footprint recorded in the \
                  registry.\n\n\
                  Read-only. The registry is read best-effort: when it cannot be \
                  opened the filesystem rows are still reported and the \
                  deployed-footprint row is omitted with a warning.",
    after_help = WRIGHT_STORAGE_AFTER_HELP
)]
pub struct StorageArgs {
    /// Emit a machine-readable JSON report instead of a table
    #[arg(long)]
    pub json: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: StorageArgs, ctx: &Context<'_>) -> Result<()> {
    // Best-effort: a damaged registry must not hide the filesystem accounting.
    let db = ctx.open_read_only().await.ok();
    crate::operations::storage::execute_storage(ctx.config, &ctx.db_path, db.as_ref(), args.json)
        .await
}
