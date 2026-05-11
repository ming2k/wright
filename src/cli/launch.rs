use std::path::PathBuf;

use clap::Args;

pub const LAUNCH_AFTER_HELP: &str = "\
Examples:
  wright launch --root /mnt/new --group ./groups/core.toml
  wright launch --root /mnt/new --plans ./plans bash coreutils glibc
  wright launch --root /mnt/new --plans ./plans @core
  wright launch --root /mnt/new @base @maintenance @desktop

Launch fills a target root from a group manifest or from explicit plan names.
When --group is given, the manifest names the plans to resolve, build, and
install.  When positional arguments are given, they are plan names or group
names prefixed with '@'.  If --plans is omitted, the default plans directory
from wright.toml is used.  Re-running launch on the same root converges drift
rather than erroring.";

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// Path to a `group.toml` file. Mutually exclusive with --plans.
    #[arg(long, value_name = "FILE", conflicts_with = "plans")]
    pub group: Option<PathBuf>,

    /// Source path: take plans from this directory and apply them into --root.
    /// Positional arguments are plan names, or group names prefixed with '@'.
    #[arg(long, value_name = "DIR", conflicts_with = "group")]
    pub plans: Option<PathBuf>,

    /// Plan or group names to launch when using --plans.
    /// Names starting with '@' are resolved as groups under the plans directory.
    #[arg(value_name = "TARGET")]
    pub plan_targets: Vec<String>,

    /// Print install order and config actions without writing anything.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Rebuild and reinstall parts that already exist in the target root.
    #[arg(long, short = 'f')]
    pub force: bool,
}
