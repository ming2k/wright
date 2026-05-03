use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

const WRIGHT_AFTER_HELP: &str = "\
Workflows:
  Install from archive catalogue:   wright install zlib
  Install from archive:     wright install ./zlib-1.3.1-1-x86_64.wright.tar.zst
  Apply an assembly:        wright apply @base
  Upgrade everything:       wright sysupgrade
  Inspect dependencies:     wright resolve zlib --tree
  Change install reason:    wright mark zlib --as-dependency

Use `wright build` to build parts from plans.";
const WRIGHT_INSTALL_AFTER_HELP: &str = "\
Examples:
  wright install zlib
  wright install zlib openssl
  wright install ./zlib-1.3.1-1-x86_64.wright.tar.zst";
const WRIGHT_APPLY_AFTER_HELP: &str = "\
Examples:
  wright apply @base
  wright apply @base @devel
  wright apply ./plans/bash
  wright apply gcc
  wright apply gcc --match=all";
const WRIGHT_UPGRADE_AFTER_HELP: &str = "\
Examples:
  wright upgrade zlib
  wright upgrade zlib --version=1.3.1
  wright upgrade ./zlib-1.3.1-1-x86_64.wright.tar.zst";
const WRIGHT_REMOVE_AFTER_HELP: &str = "\
Examples:
  wright remove zlib
  wright remove zlib --recursive
  wright remove zlib --cascade";
const WRIGHT_SYSUPGRADE_AFTER_HELP: &str = "\
Examples:
  wright sysupgrade
  wright sysupgrade --dry-run";
const WRIGHT_LIST_AFTER_HELP: &str = "\
Examples:
  wright list
  wright list -l
  wright list --roots
  wright list --orphans
  wright list --assumed

By default only part names are printed (one per line), suitable for piping.
Use -l/--long to show origin, version, release, and architecture.";
const WRIGHT_MARK_AFTER_HELP: &str = "\
Examples:
  wright mark zlib --as-dependency
  wright mark openssl --as-manual";
const WRIGHT_QUERY_AFTER_HELP: &str = "\
Examples:
  wright query zlib";
const WRIGHT_SEARCH_AFTER_HELP: &str = "\
Examples:
  wright search ssl
  wright search python

This searches installed parts only.";
const WRIGHT_FILES_AFTER_HELP: &str = "\
Examples:
  wright files zlib";
const WRIGHT_OWNER_AFTER_HELP: &str = "\
Examples:
  wright owner /usr/bin/awk
  wright owner /usr/lib/libz.so";
const WRIGHT_VERIFY_AFTER_HELP: &str = "\
Examples:
  wright verify
  wright verify zlib";
const WRIGHT_ASSUME_AFTER_HELP: &str = "\
Examples:
  wright assume glibc 2.41
  wright assume gcc 15.1.0";
const WRIGHT_UNASSUME_AFTER_HELP: &str = "\
Examples:
  wright unassume glibc";
const WRIGHT_HISTORY_AFTER_HELP: &str = "\
Examples:
  wright history
  wright history zlib";

#[derive(Parser)]
#[command(
    name = "wright",
    about = "Manage parts installed on a Wright system",
    long_about = "Manage parts installed on a Wright system.\n\nUse `wright` for system state: install, upgrade, remove, inspect, and verify parts under a target root.",
    after_help = WRIGHT_AFTER_HELP,
    version,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Alternate root directory for file operations
    #[arg(long, global = true, help_heading = "Global Options")]
    pub root: Option<PathBuf>,

    /// Path to config file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub config: Option<PathBuf>,

    /// Path to database file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub db: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = ArgAction::Count, help_heading = "Global Options")]
    pub verbose: u8,

    /// Reduce log output (show warnings/errors only)
    #[arg(long, global = true, help_heading = "Global Options")]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Install parts from local parts or the local archive catalogue
    #[command(
        long_about = "Install parts from archive files or locally registered part names.\n\nParts explicitly named by the user are marked as explicit installs. Dependencies pulled in automatically are marked as dependency installs.",
        after_help = WRIGHT_INSTALL_AFTER_HELP
    )]
    Install {
        /// Part files or locally registered part names
        #[arg(value_name = "PART")]
        parts: Vec<String>,

        /// Force reinstall even if already installed
        #[arg(long)]
        force: bool,

        /// Skip dependency resolution
        #[arg(long)]
        nodeps: bool,
    },
    /// Build and apply plan-driven installs/upgrades for plans or assemblies
    #[command(
        long_about = "Apply plans or assemblies to the local system.\n\nTargets may be plan names, plan directories, or `@assembly` references. Wright is the high-level source-first combo command: it resolves requested targets, automatically pulls in dependencies that are missing or outdated under the selected match policy, builds what is needed in dependency waves, and installs each completed wave onto the live system. Use it for natural plan-driven install and upgrade workflows.",
        after_help = WRIGHT_APPLY_AFTER_HELP
    )]
    Apply {
        /// Plan names, plan directories, or @assemblies
        #[arg(value_name = "TARGET")]
        targets: Vec<String>,

        /// Resume a previous apply session using the same targets and scope flags.
        /// Optionally pass the session hash printed on failure.
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        resume: Option<String>,

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
        deps: Option<crate::cli::resolve::DomainArg>,

        /// Expand reverse dependents (rdeps) for installed parts.
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
        rdeps: Option<crate::cli::resolve::DomainArg>,

        /// Match policy for filtering based on installation state.
        /// Can be specified multiple times. If omitted, `apply` defaults to
        /// `outdated`, so missing and changed dependencies are added
        /// automatically while already-converged ones are skipped.
        #[arg(long = "match", alias = "match-policies", value_enum)]
        match_policies: Vec<crate::cli::resolve::MatchPolicyArg>,

        /// Maximum expansion depth. `0` means unlimited.
        #[arg(long)]
        depth: Option<usize>,

        /// Force a clean rebuild and reinstall even if matching parts already exist
        #[arg(long, short = 'f')]
        force: bool,

        /// Preview what would be built and installed without making any changes
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
    /// Upgrade installed parts by name or from archive files
    #[command(
        long_about = "Upgrade installed parts by name or from archive files.\n\nWhen given a part name, `wright` resolves the latest locally registered version from the archive catalogue. When given an archive path, it upgrades directly from that file.",
        after_help = WRIGHT_UPGRADE_AFTER_HELP
    )]
    Upgrade {
        /// Part names or archive files to upgrade
        #[arg(required = true, value_name = "PART")]
        parts: Vec<String>,

        /// Force upgrade even if version is not newer
        #[arg(long)]
        force: bool,

        /// Target a specific version (implies --force for downgrades)
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove installed parts
    #[command(
        long_about = "Remove installed parts by name or plan.\n\nBy default, removal is blocked when another installed part depends on the target. Use `--recursive` to remove dependents too, or `--force` to bypass safety checks.",
        after_help = WRIGHT_REMOVE_AFTER_HELP
    )]
    Remove {
        /// Part names to remove (or plan names when using --plan)
        #[arg(required = true, value_name = "PART")]
        parts: Vec<String>,

        /// Force removal even if other parts depend on this one
        #[arg(long)]
        force: bool,

        /// Recursively remove all parts that depend on the target
        #[arg(long, short)]
        recursive: bool,

        /// Also remove orphan dependencies (auto-installed deps no longer needed)
        #[arg(long, short = 'c')]
        cascade: bool,

        /// Treat arguments as plan names and remove all parts from those plans
        #[arg(long)]
        plan: bool,
    },
    /// List installed parts
    #[command(
        long_about = "List installed parts.\n\nUse filters to narrow the output to root parts, assumed external parts, orphaned dependency installs, or parts from a specific plan.",
        after_help = WRIGHT_LIST_AFTER_HELP
    )]
    List {
        /// Show origin, version, release, and architecture
        #[arg(long, short)]
        long: bool,
        /// Show only top-level (root) parts with no installed dependents
        #[arg(long, short)]
        roots: bool,
        /// Show only assumed (externally provided) parts
        #[arg(long, short)]
        assumed: bool,
        /// Show only orphan parts (auto-installed deps no longer needed)
        #[arg(long, short)]
        orphans: bool,
        /// Show only parts from a specific plan
        #[arg(long, short)]
        plan: Option<String>,
    },
    /// Show detailed part information
    #[command(
        long_about = "Show detailed metadata for an installed part.",
        after_help = WRIGHT_QUERY_AFTER_HELP
    )]
    Query {
        /// Part name
        #[arg(value_name = "PART")]
        part: String,
    },
    /// Search installed parts by keyword
    #[command(
        long_about = "Search installed parts by keyword.\n\nMatches are taken from installed part names and descriptions.",
        after_help = WRIGHT_SEARCH_AFTER_HELP
    )]
    Search {
        /// Search keyword
        keyword: String,
    },
    /// List files owned by a part
    #[command(
        long_about = "List files recorded as owned by an installed part.",
        after_help = WRIGHT_FILES_AFTER_HELP
    )]
    Files {
        /// Part name
        #[arg(value_name = "PART")]
        part: String,
    },
    /// Find which part owns a file
    #[command(
        long_about = "Find which installed part owns a given file path.",
        after_help = WRIGHT_OWNER_AFTER_HELP
    )]
    Owner {
        /// File path
        file: String,
    },
    /// Verify installed part file integrity (SHA-256 checksums)
    #[command(
        long_about = "Verify installed part file integrity using recorded SHA-256 checksums.\n\nPass a part name to verify one part, or omit it to verify all installed parts.",
        after_help = WRIGHT_VERIFY_AFTER_HELP
    )]
    Verify {
        /// Part name; omit to verify all installed parts
        #[arg(value_name = "PART")]
        part: Option<String>,
    },
    /// Perform a full system health check (integrity, dependencies, file conflicts, shadows)
    Doctor,
    /// Mark a part as externally provided to satisfy dependency checks
    #[command(
        long_about = "Mark a part as externally provided so dependency checks consider it satisfied.\n\nPass a name and version as arguments, pipe 'name version' lines, or use --file for bulk bootstrap.",
        after_help = WRIGHT_ASSUME_AFTER_HELP
    )]
    Assume {
        /// Part name (omit if piping or using --file)
        #[arg(value_name = "PART")]
        name: Option<String>,
        /// Part version (omit if piping or using --file)
        version: Option<String>,
        /// Read 'name version' pairs from a file (one per line)
        #[arg(long, value_name = "FILE")]
        file: Option<std::path::PathBuf>,
    },
    /// Remove an assumed (externally provided) part record
    #[command(
        long_about = "Remove an assumed part record created with `wright assume`.",
        after_help = WRIGHT_UNASSUME_AFTER_HELP
    )]
    Unassume {
        /// Part name
        #[arg(value_name = "PART")]
        name: String,
    },
    /// Change the install origin of a part
    #[command(
        long_about = "Change the install origin of a part.\n\nThis controls whether a part is treated as explicitly installed or as a dependency. Marking a part as a dependency makes it eligible for orphan cleanup.",
        after_help = WRIGHT_MARK_AFTER_HELP
    )]
    Mark {
        /// Part names
        #[arg(required = true, value_name = "PART")]
        parts: Vec<String>,
        /// Mark as a dependency install
        #[arg(long, group = "origin")]
        as_dependency: bool,
        /// Mark as an explicit (manual) install
        #[arg(long, group = "origin")]
        as_manual: bool,
    },
    /// Show part transaction history (install, upgrade, remove)
    #[command(
        long_about = "Show part transaction history.\n\nPass a part name to limit the history to one part, or omit it to show all recorded transactions.",
        after_help = WRIGHT_HISTORY_AFTER_HELP
    )]
    History {
        /// Part name; omit to show all history
        #[arg(value_name = "PART")]
        part: Option<String>,
    },
    /// Upgrade all installed parts to latest available versions
    #[command(
        long_about = "Upgrade all installed parts to the latest versions available in the local archive catalogue.\n\nUse `--dry-run` to preview the transaction without making any changes.",
        after_help = WRIGHT_SYSUPGRADE_AFTER_HELP
    )]
    Sysupgrade {
        /// Preview what would be upgraded without actually doing it
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
}
