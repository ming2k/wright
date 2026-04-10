pub mod build;
pub mod inventory;
pub mod system;

use std::path::PathBuf;

use anyhow::Result;

use crate::cli::Cli;
use crate::config::GlobalConfig;
use crate::inventory::resolver::LocalResolver;

/// Dispatch the parsed CLI command to the appropriate handler.
pub fn dispatch(
    cli: Cli,
    config: &GlobalConfig,
    db_path: PathBuf,
    root_dir: PathBuf,
) -> Result<()> {
    match cli.command {
        crate::cli::Commands::System(sys_cmd) => {
            system::execute(sys_cmd, config, &db_path, &root_dir, cli.verbose, cli.quiet)
        }
        crate::cli::Commands::Build(args) => {
            build::execute_build(args, config, cli.verbose, cli.quiet)
        }
        crate::cli::Commands::Plan(cmd) => build::execute_plan(cmd, config),
        crate::cli::Commands::Inventory(cmd) => inventory::execute(cmd, config),
    }
}

/// Helper function to setup the local resolver for commands that need it.
pub(crate) fn setup_local_resolver(config: &GlobalConfig) -> Result<LocalResolver> {
    let mut resolver = crate::builder::orchestrator::setup_resolver(config)?;
    resolver.add_search_dir(config.general.components_dir.clone());
    if let Ok(cwd) = std::env::current_dir() {
        resolver.add_search_dir(cwd);
    }
    Ok(resolver)
}
