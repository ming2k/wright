use clap::Args;

use crate::cli::common::{DomainArg, MatchPolicyArg};

const WRIGHT_INSTALL_AFTER_HELP: &str = "\
Examples:
  wright install zlib
  wright install zlib --invalidate
  wright install zlib openssl
  wright install ./plans/zlib
  wright install --path ./zlib-1.3.1-1-x86_64.wright.tar.zst";

const WRIGHT_INSTALL_AFTER_HELP_FULL: &str = "\
Examples:
  wright install zlib
  wright install zlib openssl
  wright install ./plans/bash
  wright install @core
  wright install gcc --match=all";

const WRIGHT_UPGRADE_AFTER_HELP: &str = "\
Examples:
  wright upgrade zlib
  wright upgrade zlib openssl
  wright upgrade all
  wright upgrade all --force";

const WRIGHT_REMOVE_AFTER_HELP: &str = "\
Examples:
  wright remove zlib
  wright remove zlib --recursive
  wright remove zlib --cascade";

const WRIGHT_ASSUME_AFTER_HELP: &str = "\
Examples:
  wright assume glibc 2.41
  wright assume gcc 15.1.0";

const WRIGHT_UNASSUME_AFTER_HELP: &str = "\
Examples:
  wright unassume glibc";

#[derive(Args)]
#[command(
    long_about = "Merge part archives into the target root.\n\nBy default, arguments are plan names or plan directories. Wright reads each plan manifest, derives the expected output archive names, and merges those archives from parts_dir into the system. Use --path to merge explicit archive paths instead. Runtime dependencies are checked for warnings and recorded in the database, but missing runtime dependencies do not block merging.",
    after_help = WRIGHT_INSTALL_AFTER_HELP
)]
pub struct MergeArgs {
    /// Plan names/directories, or archive paths when using --path
    #[arg(value_name = "TARGET")]
    pub parts: Vec<String>,

    /// Force redeploy even if already deployed
    #[arg(long)]
    pub force: bool,

    /// Skip runtime dependency warnings
    #[arg(long)]
    pub nodeps: bool,

    /// Treat arguments and stdin as explicit archive paths
    #[arg(long)]
    pub path: bool,
}

#[derive(Args)]
#[command(
    long_about = "Install plans to the local system.\n\nTargets may be plan names, plan directories, or folio names prefixed with '@'. Wright is the high-level source-first combo command: it resolves requested targets, automatically pulls in all dependencies (build, link, and runtime) that are missing or outdated under the selected match policy, forges what is needed in dependency waves, seals outputs, and merges each completed wave onto the live system. Use it for natural plan-driven install and upgrade workflows.",
    after_help = WRIGHT_INSTALL_AFTER_HELP_FULL
)]
pub struct InstallArgs {
    /// Plan names, plan directories, or folio names prefixed with '@'
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Expand dependencies.
    /// `link` follows ABI-sensitive link dependencies.
    /// `runtime` follows runtime dependencies.
    /// `build` follows build dependencies.
    /// `all` follows all dependencies.
    #[arg(
        short = 'd',
        long = "deps",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "all"
    )]
    pub deps: Option<DomainArg>,

    /// Expand reverse dependents (rdeps) for deployed parts.
    /// `link` follows ABI-sensitive link dependents.
    /// `runtime` follows runtime dependents.
    /// `build` follows build dependents.
    /// `all` follows all dependents.
    #[arg(
        short = 'r',
        long = "rdeps",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "link"
    )]
    pub rdeps: Option<DomainArg>,

    /// Match policy for filtering based on deployment state.
    /// Can be specified multiple times. If omitted, `install` defaults to
    /// `outdated`, so missing and changed dependencies are added
    /// automatically while already-converged ones are skipped.
    #[arg(long = "match", alias = "match-policies", value_enum)]
    pub match_policies: Vec<MatchPolicyArg>,

    /// Maximum expansion depth. `0` means unlimited.
    #[arg(long)]
    pub depth: Option<usize>,

    /// Force a clean reforge and redeploy even if matching parts already exist
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Preview what would be forged and deployed without making any changes
    #[arg(long, short = 'n')]
    pub dry_run: bool,
}

#[derive(Args)]
#[command(
    long_about = "Upgrade plans to the latest version.\n\nWhen given plan names, `wright` checks if the plan has a newer version than what is deployed, then resolves, forges, seals, and deploys it along with any installed parts that link-depend on it (to ensure ABI consistency). Use `all` to check every installed plan for updates.\n\nFor archive-based upgrades, use `wright merge --force`.",
    after_help = WRIGHT_UPGRADE_AFTER_HELP
)]
pub struct UpgradeArgs {
    /// Plan names to upgrade, or `all` to upgrade all outdated plans
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Force reforge and redeploy even if the plan version matches
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Maximum depth for reverse dependency expansion. `0` means unlimited.
    #[arg(long)]
    pub depth: Option<usize>,
}

#[derive(Args)]
#[command(
    long_about = "Remove deployed parts by name.\n\nBy default, removal is blocked when another deployed part depends on the target. Use `--recursive` to remove dependents too, or `--force` to bypass safety checks.",
    after_help = WRIGHT_REMOVE_AFTER_HELP
)]
pub struct RemoveArgs {
    /// Part names to remove
    #[arg(required = true, value_name = "PART")]
    pub parts: Vec<String>,

    /// Force removal even if other parts depend on this one
    #[arg(long)]
    pub force: bool,

    /// Recursively remove all parts that depend on the target
    #[arg(long, short)]
    pub recursive: bool,

    /// Also remove orphan dependencies (auto-deployed deps no longer needed)
    #[arg(long, short = 'c')]
    pub cascade: bool,
}

#[derive(Args)]
#[command(
    long_about = "Mark a part as externally provided so dependency checks consider it satisfied.\n\nPass a name and version as arguments, pipe 'name version' lines, or use --file for bulk bootstrap.",
    after_help = WRIGHT_ASSUME_AFTER_HELP
)]
pub struct AssumeArgs {
    /// Part name (omit if piping or using --file)
    #[arg(value_name = "PART")]
    pub name: Option<String>,
    /// Part version (omit if piping or using --file)
    pub version: Option<String>,
    /// Read 'name version' pairs from a file (one per line)
    #[arg(long, value_name = "FILE")]
    pub file: Option<std::path::PathBuf>,
}

#[derive(Args)]
#[command(
    long_about = "Remove an assumed part record created with `wright assume`.",
    after_help = WRIGHT_UNASSUME_AFTER_HELP
)]
pub struct UnassumeArgs {
    /// Part name
    #[arg(value_name = "PART")]
    pub name: String,
}
