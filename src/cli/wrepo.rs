use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

const WREPO_AFTER_HELP: &str = "\
Workflows:
  Index local archives:    wrepo sync
  Search indexed parts:    wrepo search zlib
  Show all versions:       wrepo list zlib
  Add a source:            wrepo source add local --path=/srv/wright/repo

Use `wrepo` to maintain repository metadata and source configuration.";
const WREPO_SYNC_AFTER_HELP: &str = "\
Examples:
  wrepo sync
  wrepo sync ./components";
const WREPO_LIST_AFTER_HELP: &str = "\
Examples:
  wrepo list
  wrepo list zlib";
const WREPO_SEARCH_AFTER_HELP: &str = "\
Examples:
  wrepo search zlib
  wrepo search ssl";
const WREPO_REMOVE_AFTER_HELP: &str = "\
Examples:
  wrepo remove zlib 1.3.1
  wrepo remove zlib 1.3.1-2 --purge";
const WREPO_SOURCE_AFTER_HELP: &str = "\
Examples:
  wrepo source list
  wrepo source add local --path=/srv/wright/repo
  wrepo source remove local";
const WREPO_SOURCE_ADD_AFTER_HELP: &str = "\
Examples:
  wrepo source add local --path=/srv/wright/repo
  wrepo source add cache --path=./repo --priority=200";
const WREPO_SOURCE_REMOVE_AFTER_HELP: &str = "\
Examples:
  wrepo source remove local";
const WREPO_SOURCE_LIST_AFTER_HELP: &str = "\
Examples:
  wrepo source list";

#[derive(Parser)]
#[command(
    name = "wrepo",
    about = "Manage Wright repository indexes and sources",
    long_about = "Manage Wright repository indexes and sources.\n\nUse `wrepo` to index local archives, search available parts, remove repository entries, and configure repository sources used by the resolver.",
    after_help = WREPO_AFTER_HELP,
    version,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Path to config file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub config: Option<PathBuf>,

    /// Increase verbosity (-v or -vv)
    #[arg(long, short, action = ArgAction::Count, global = true, help_heading = "Global Options")]
    pub verbose: u8,

    /// Suppress non-error output
    #[arg(long, short, global = true, help_heading = "Global Options")]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Import parts from a directory of .wright.tar.zst archives
    #[command(
        long_about = "Import parts from a directory of `.wright.tar.zst` archives into the local repository index.\n\nIf no directory is given, `wrepo` indexes the configured `components_dir`.",
        after_help = WREPO_SYNC_AFTER_HELP
    )]
    Sync {
        /// Directory containing .wright.tar.zst files (default: components_dir)
        dir: Option<PathBuf>,
    },
    /// List parts available in the repository
    #[command(
        long_about = "List parts available in the local repository index.\n\nPass a part name to show all indexed versions for that part.",
        after_help = WREPO_LIST_AFTER_HELP
    )]
    List {
        /// Show all versions of a specific part
        name: Option<String>,
    },
    /// Search available parts by keyword
    #[command(
        long_about = "Search available parts by keyword.\n\nMatches are taken from indexed part names and descriptions.",
        after_help = WREPO_SEARCH_AFTER_HELP
    )]
    Search {
        /// Search keyword (matches name and description)
        keyword: String,
    },
    /// Remove a part entry from the repository
    #[command(
        long_about = "Remove a part entry from the local repository index.\n\nUse `--purge` to also delete the corresponding archive file from disk.",
        after_help = WREPO_REMOVE_AFTER_HELP
    )]
    Remove {
        /// Part name
        name: String,
        /// Part version (e.g. "1.2.3" or "1.2.3-2" for specific release)
        version: String,
        /// Also delete the archive file from disk
        #[arg(long)]
        purge: bool,
    },
    /// Manage repository sources
    #[command(
        long_about = "Manage repository sources used by the resolver.\n\nSources define where part metadata and archives are discovered when installing or upgrading by part name.",
        after_help = WREPO_SOURCE_AFTER_HELP
    )]
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
}

#[derive(Subcommand)]
pub enum SourceAction {
    /// Add a new repository source
    #[command(
        long_about = "Add a new repository source.\n\nSources are identified by name and point to a local directory path. Higher priority sources are preferred during resolution.",
        after_help = WREPO_SOURCE_ADD_AFTER_HELP
    )]
    Add {
        /// Unique source name
        name: String,

        /// Local directory path
        #[arg(long)]
        path: PathBuf,

        /// Priority (higher = preferred)
        #[arg(long, default_value = "100")]
        priority: i32,
    },
    /// Remove a repository source
    #[command(
        long_about = "Remove a configured repository source by name.",
        after_help = WREPO_SOURCE_REMOVE_AFTER_HELP
    )]
    Remove {
        /// Source name to remove
        name: String,
    },
    /// List configured repository sources
    #[command(
        long_about = "List configured repository sources in resolver priority order.",
        after_help = WREPO_SOURCE_LIST_AFTER_HELP
    )]
    List,
}
