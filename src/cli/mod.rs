pub mod build;
pub mod common;
pub mod maintenance;
pub mod query;
pub mod system;

use clap::{ArgAction, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "wright",
    about = "Declarative, extensible, sandboxed Linux package manager",
    long_about = "Declarative, extensible, sandboxed Linux package manager\n\n\
                  Command groups:\n  \
                  System Management    install, remove, upgrade, merge, assume, unassume\n  \
                  Query & Inspection   list, files, check, history, doctor\n  \
                  Build & Packaging    build, lint, launch\n  \
                  Cache & Maintenance  prune",
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

    /// Path to database file
    #[arg(long, help_heading = "Global Options")]
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
    // ── System Management ──────────────────────────────────────────
    /// Merge plan archives into the target root
    #[command(display_order = 1)]
    Merge(system::MergeArgs),

    /// Install plans: resolve, forge, seal, and merge with full lifecycle
    #[command(display_order = 2)]
    Install(system::InstallArgs),

    /// Upgrade plans: resolve, rebuild, seal, and deploy with reverse dependency expansion
    #[command(display_order = 3)]
    Upgrade(system::UpgradeArgs),

    /// Remove deployed parts
    #[command(display_order = 4)]
    Remove(system::RemoveArgs),

    /// Mark a part as externally provided to satisfy dependency checks
    #[command(display_order = 5)]
    Assume(system::AssumeArgs),

    /// Remove an assumed (externally provided) part record
    #[command(display_order = 6)]
    Unassume(system::UnassumeArgs),

    // ── Query & Inspection ─────────────────────────────────────────
    /// List deployed parts
    #[command(display_order = 11)]
    List(query::ListArgs),

    /// List files owned by a part
    #[command(display_order = 12)]
    Files(query::FilesArgs),

    /// Perform system health checks
    #[command(display_order = 13)]
    Check(query::CheckArgs),

    /// Show part transaction history (deploy, upgrade, remove)
    #[command(display_order = 14)]
    History(query::HistoryArgs),

    /// Diagnose system and archive health issues
    #[command(display_order = 15)]
    Doctor(query::DoctorArgs),

    // ── Build & Packaging ──────────────────────────────────────────
    /// Forge parts from plans
    #[command(display_order = 21)]
    Build(build::BuildArgs),

    /// Verify the syntax and logical integrity of plan files
    #[command(display_order = 22)]
    Lint(build::LintArgs),

    /// Fill a target root from a folio manifest or from plans
    #[command(display_order = 23)]
    Launch(build::LaunchArgs),

    // ── Cache & Maintenance ────────────────────────────────────────
    /// Prune stale archives from the parts directory
    #[command(display_order = 31)]
    Prune(maintenance::PruneArgs),
}
