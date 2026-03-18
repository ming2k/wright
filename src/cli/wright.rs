use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

const WRIGHT_AFTER_HELP: &str = "\
Workflows:
  Install from repository:  wright install zlib
  Install from archive:     wright install ./zlib-1.3.1-1-x86_64.wright.tar.zst
  Upgrade everything:       wright sysupgrade
  Inspect dependencies:     wright deps zlib --reverse

Use `wrepo` to manage repository indexes and sources.
Use `wbuild` to build parts from plans.";
const WRIGHT_INSTALL_AFTER_HELP: &str = "\
Examples:
  wright install zlib
  wright install zlib openssl
  wright install @base-devel
  wright install ./zlib-1.3.1-1-x86_64.wright.tar.zst";
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
const WRIGHT_DEPS_AFTER_HELP: &str = "\
Examples:
  wright deps zlib
  wright deps zlib --reverse
  wright deps --all --depth=2
  wright deps zlib --prefix=depth

This command reads installed dependency metadata from the local part database,
which is populated from archive `.PARTINFO` metadata during install/upgrade.
It does not inspect `plan.toml` files.";
const WRIGHT_SYSUPGRADE_AFTER_HELP: &str = "\
Examples:
  wright sysupgrade
  wright sysupgrade --dry-run";
const WRIGHT_LIST_AFTER_HELP: &str = "\
Examples:
  wright list
  wright list --roots
  wright list --orphans
  wright list --assumed";
const WRIGHT_QUERY_AFTER_HELP: &str = "\
Examples:
  wright query zlib";
const WRIGHT_SEARCH_AFTER_HELP: &str = "\
Examples:
  wright search ssl
  wright search python";
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

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum PrefixModeArg {
    Indent,
    Depth,
    None,
}

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
    /// Install parts from .wright.tar.zst files, part names, or @kits
    #[command(
        long_about = "Install parts from archive files, repository part names, or `@kit` references.\n\nParts explicitly named by the user are marked as explicit installs. Dependencies pulled in automatically are marked as dependency installs.",
        after_help = WRIGHT_INSTALL_AFTER_HELP
    )]
    Install {
        /// Part files, part names, or @kit names
        #[arg(required = true, value_name = "PART")]
        parts: Vec<String>,

        /// Force reinstall even if already installed
        #[arg(long)]
        force: bool,

        /// Skip dependency resolution
        #[arg(long)]
        nodeps: bool,
    },
    /// Upgrade installed parts by name or from archive files
    #[command(
        long_about = "Upgrade installed parts by name or from archive files.\n\nWhen given a part name, `wright` resolves the latest available version from configured sources. When given an archive path, it upgrades directly from that file.",
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
        long_about = "Remove installed parts by name.\n\nBy default, removal is blocked when another installed part depends on the target. Use `--recursive` to remove dependents too, or `--force` to bypass safety checks.",
        after_help = WRIGHT_REMOVE_AFTER_HELP
    )]
    Remove {
        /// Part names to remove
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
    },
    /// Analyze installed part dependency relationships from the local part database
    #[command(
        long_about = "Analyze dependency relationships among installed parts.\n\nThis command reads the local installed-part database, which is populated from archive `.PARTINFO` metadata during install and upgrade. By default it shows forward dependencies. Use `--reverse` to see what depends on a part, or `--all` to inspect the whole installed graph. It does not read `plan.toml` files.",
        after_help = WRIGHT_DEPS_AFTER_HELP
    )]
    Deps {
        /// Part name
        #[arg(value_name = "PART")]
        part: Option<String>,

        /// Show reverse dependencies (what depends on this part)
        #[arg(long, short)]
        reverse: bool,

        /// Maximum depth to display (0 = unlimited)
        #[arg(long, short, default_value = "0")]
        depth: usize,

        /// Filter output to only show matching part names
        #[arg(long, short)]
        filter: Option<String>,

        /// Show dependency tree for all installed parts
        #[arg(long, short)]
        all: bool,

        /// Output prefix style: indent (tree), depth (flat + depth number), none (bare names)
        #[arg(long, value_enum, default_value_t = PrefixModeArg::Indent)]
        prefix: PrefixModeArg,

        /// Hide the subtree of the named part (can be repeated)
        #[arg(long, action = ArgAction::Append)]
        prune: Vec<String>,
    },
    /// List installed parts
    #[command(
        long_about = "List installed parts.\n\nUse filters to narrow the output to root parts, assumed external parts, or orphaned dependency installs.",
        after_help = WRIGHT_LIST_AFTER_HELP
    )]
    List {
        /// Show only top-level (root) parts with no installed dependents
        #[arg(long, short)]
        roots: bool,
        /// Show only assumed (externally provided) parts
        #[arg(long, short)]
        assumed: bool,
        /// Show only orphan parts (auto-installed deps no longer needed)
        #[arg(long, short)]
        orphans: bool,
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
        long_about = "Mark a part as externally provided so dependency checks consider it satisfied.\n\nThis is useful when bootstrapping a system that already contains core parts not installed through `wright`.",
        after_help = WRIGHT_ASSUME_AFTER_HELP
    )]
    Assume {
        /// Part name
        #[arg(value_name = "PART")]
        name: String,
        /// Part version
        version: String,
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
        long_about = "Upgrade all installed parts to the latest versions available from configured sources.\n\nUse `--dry-run` to preview the transaction without making any changes.",
        after_help = WRIGHT_SYSUPGRADE_AFTER_HELP
    )]
    Sysupgrade {
        /// Preview what would be upgraded without actually doing it
        #[arg(long, short = 'n')]
        dry_run: bool,
    },
}
