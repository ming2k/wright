use std::path::Path;

use crate::cli::maintenance::PruneArgs;
use crate::config::GlobalConfig;
use crate::error::Result;

pub async fn dispatch_prune(args: PruneArgs, config: &GlobalConfig, db_path: &Path) -> Result<()> {
    crate::operations::prune::execute_prune(config, db_path, args.latest, args.apply).await
}
