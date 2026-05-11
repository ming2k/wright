use std::path::Path;

use crate::error::Result;

use crate::cli::launch::LaunchArgs;
use crate::config::GlobalConfig;

pub async fn execute_launch(
    args: LaunchArgs,
    config: &GlobalConfig,
    db_path: &Path,
    root_dir: &Path,
    verbose: u8,
    quiet: bool,
) -> Result<()> {
    let request = crate::operations::launch::LaunchRequest {
        group: args.group,
        plans: args.plans,
        plan_targets: args.plan_targets,
        dry_run: args.dry_run,
        force: args.force,
    };
    crate::operations::launch::execute_launch(request, config, db_path, root_dir, verbose, quiet)
        .await
}
