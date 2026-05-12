use clap::Args;
use std::path::PathBuf;

const WRIGHT_LIST_AFTER_HELP: &str = "\
Examples:
  wright list
  wright list -l
  wright list --roots
  wright list --orphans
  wright list --assumed

By default only part names are printed (one per line), suitable for piping.
Use -l/--long to show origin, version, release, and architecture.";

const WRIGHT_FILES_AFTER_HELP: &str = "\
Examples:
  wright files zlib";

const WRIGHT_CHECK_AFTER_HELP: &str = "\
Examples:
  wright check
  wright check zlib
  wright check --deep
  wright check --integrity-only";

const WRIGHT_HISTORY_AFTER_HELP: &str = "\
Examples:
  wright history
  wright history zlib";

const WRIGHT_DOCTOR_AFTER_HELP: &str = "\
Examples:
  wright doctor";

#[derive(Args)]
#[command(
    long_about = "List deployed parts.\n\nUse filters to narrow the output to root parts, assumed external parts, or orphaned dependency deploys.",
    after_help = WRIGHT_LIST_AFTER_HELP
)]
pub struct ListArgs {
    /// Show origin, version, release, and architecture
    #[arg(long, short)]
    pub long: bool,
    /// Show only top-level (root) parts with no deployed dependents
    #[arg(long, short)]
    pub roots: bool,
    /// Show only assumed (externally provided) parts
    #[arg(long, short)]
    pub assumed: bool,
    /// Show only orphan parts (auto-deployed deps no longer needed)
    #[arg(long, short)]
    pub orphans: bool,
}

#[derive(Args)]
#[command(
    long_about = "List files recorded as owned by a deployed part.",
    after_help = WRIGHT_FILES_AFTER_HELP
)]
pub struct FilesArgs {
    /// Part name
    #[arg(value_name = "PART")]
    pub part: String,
}

#[derive(Args)]
#[command(
    long_about = "Run system health checks covering database integrity, file conflicts, \
                  shadowed files, and runtime dependency resolution.\n\n\
                  With --deep, walk each deployed part's ELF binaries and \
                  verify their DT_NEEDED entries against the deployed \
                  file ownership table. This catches forgotten declarations \
                  that the registry-level check would miss.\n\n\
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
    #[arg(long, conflicts_with = "deep")]
    pub integrity_only: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[derive(Args)]
#[command(
    long_about = "Show part transaction history.\n\nPass a part name to limit the history to one part, or omit it to show all recorded transactions.",
    after_help = WRIGHT_HISTORY_AFTER_HELP
)]
pub struct HistoryArgs {
    /// Part name; omit to show all history
    #[arg(value_name = "PART")]
    pub part: Option<String>,
}

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
