pub mod build;
pub mod prune;
pub mod resolve;
pub mod system;

use clap::{ArgAction, Parser, Subcommand};
use std::path::PathBuf;

use build::BUILD_AFTER_HELP;
use resolve::RESOLVE_AFTER_HELP;

#[derive(Parser)]
#[command(
    name = "wright",
    about = "Declarative, extensible, sandboxed Linux package manager",
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
    // --- System Management ---
    #[command(flatten)]
    System(system::Commands),

    // --- Build & Development ---
    /// Build parts from plans
    #[command(after_help = BUILD_AFTER_HELP)]
    Build(build::BuildArgs),

    /// Resolve targets and expand their dependency graph
    #[command(after_help = RESOLVE_AFTER_HELP)]
    Resolve(resolve::ResolveArgs),

    /// Prune local archive inventory and stale archives
    Prune(prune::PruneArgs),
}
