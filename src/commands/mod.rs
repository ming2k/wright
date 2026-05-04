pub mod build;
pub mod lint;
pub mod package;
pub mod prune;
pub mod resolve;
pub mod system;

use crate::archive::resolver::LocalResolver;
use crate::cli::Cli;
use crate::config::GlobalConfig;
use anyhow::Result;
use std::path::PathBuf;

/// Dispatch the parsed CLI command to the appropriate handler.
pub async fn dispatch(
    cli: Cli,
    config: &GlobalConfig,
    db_path: PathBuf,
    root_dir: PathBuf,
) -> Result<()> {
    match cli.command {
        crate::cli::Commands::System(sys_cmd) => {
            system::execute(
                sys_cmd,
                config,
                &db_path,
                &root_dir,
                cli.verbose,
                cli.quiet,
            )
            .await
        }
        crate::cli::Commands::Build(args) => {
            build::execute_build(args, config, cli.verbose, cli.quiet).await
        }
        crate::cli::Commands::Package(args) => {
            package::execute_package(args, config, cli.verbose, cli.quiet).await
        }
        crate::cli::Commands::Resolve(args) => {
            resolve::execute_resolve(args, config, &db_path).await
        }
        crate::cli::Commands::Lint { targets, recursive } => {
            lint::execute_lint(targets, recursive, config)
                .await
                .map_err(Into::into)
        }
        crate::cli::Commands::Prune(args) => prune::execute_prune(args, config).await,
    }
}

/// Helper function to setup the local resolver for commands that need it.
pub(crate) fn setup_local_resolver(config: &GlobalConfig) -> Result<LocalResolver> {
    crate::builder::orchestrator::setup_resolver(config).map_err(Into::into)
}
