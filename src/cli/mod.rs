pub mod wbuild;
pub mod wright;

use std::path::PathBuf;
use clap::{ArgAction, Parser, Subcommand};

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
    // --- System Management (formerly wright) ---
    #[command(flatten)]
    System(wright::Commands),

    // --- Build & Development (formerly wbuild) ---
    
    /// Build parts from plans
    ///
    /// This is the direct replacement for `wbuild run`.
    Build(wbuild::RunArgs),

    /// Manage build plans (resolve, check, fetch, checksum)
    #[command(subcommand)]
    Plan(PlanCommands),

    /// Manage the local archive inventory
    #[command(subcommand)]
    Inventory(InventoryCommands),
}

#[derive(Subcommand)]
pub enum PlanCommands {
    /// Resolve targets and expand their dependency graph
    Resolve(wbuild::ResolveArgs),
    /// Validate plan.toml files for syntax and logic errors
    Check(wbuild::CheckArgs),
    /// Download sources for plans without building
    Fetch(wbuild::FetchArgs),
    /// Compute and update SHA256 checksums in plan.toml
    Checksum(wbuild::ChecksumArgs),
}

#[derive(Subcommand)]
pub enum InventoryCommands {
    /// Prune local archive inventory and stale archives
    Prune(wbuild::PruneArgs),
}
