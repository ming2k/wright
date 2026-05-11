pub mod build;
pub mod launch;
pub mod lint;
pub mod package;
pub mod prune;
pub mod resolve;
pub mod system;

use crate::cli::Cli;
use crate::config::GlobalConfig;
use crate::part::store::LocalPartStore;
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
            system::execute(sys_cmd, config, &db_path, &root_dir, cli.verbose, cli.quiet).await
        }
        crate::cli::Commands::Build(args) => {
            build::execute_build(args, config, &db_path, cli.verbose, cli.quiet).await
        }
        crate::cli::Commands::Package(args) => {
            package::execute_package(args, config, &db_path, cli.verbose, cli.quiet).await
        }
        crate::cli::Commands::Resolve(args) => {
            resolve::execute_resolve(args, config, &db_path).await
        }
        crate::cli::Commands::Lint {
            targets,
            recursive,
            verify,
        } => lint::execute_lint(targets, recursive, verify, config)
            .await
            .map_err(Into::into),
        crate::cli::Commands::Prune(args) => prune::execute_prune(args, config, &db_path).await,

        crate::cli::Commands::Launch(args) => {
            launch::execute_launch(args, config, &db_path, &root_dir, cli.verbose, cli.quiet).await
        }
    }
}

/// Helper function to setup the local part store for commands that need it.
pub(crate) fn setup_local_part_store(config: &GlobalConfig) -> Result<LocalPartStore> {
    crate::planning::setup_part_store(config).map_err(Into::into)
}
