pub mod launch;

use std::path::Path;

use crate::cli::build::{BuildArgs, LaunchArgs, LintArgs};
use crate::config::GlobalConfig;
use crate::error::Result;

pub async fn dispatch_build(
    args: BuildArgs,
    config: &GlobalConfig,
    db_path: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    crate::operations::build::execute_build(args, config, db_path, verbose, quiet).await
}

pub async fn dispatch_lint(args: LintArgs, config: &GlobalConfig) -> Result<()> {
    crate::operations::lint::execute_lint(args.targets, args.recursive, args.verify, config)
        .await}

pub async fn dispatch_launch(
    args: LaunchArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    launch::execute_launch(args, config, db_path, root_dir, verbose, quiet).await
}
