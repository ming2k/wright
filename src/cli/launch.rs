use std::path::PathBuf;

use clap::Args;

pub const LAUNCH_AFTER_HELP: &str = "\
Examples:
  wright launch --root /mnt/new ./base.wright.pack.tar
  wright launch --root /mnt/new --plans ./plans bash coreutils glibc
  wright launch --root /mnt/new --profile minimal

Launch fills a target root from either a pack archive or from plans. The pack
path takes precedence when both are given. Re-running launch on the same root
is a convergence operation, not an error.";

pub const PACK_AFTER_HELP: &str = "\
Examples:
  wright pack ./my-base/
  wright pack ./my-base/ -o /tmp/my-base-1.wright.pack.tar
  wright pack inspect ./my-base.wright.pack.tar";

#[derive(Args, Debug)]
pub struct LaunchArgs {
    /// Path to a `.wright.pack.tar` file to launch from. Mutually exclusive with
    /// `--plans` and `--profile`; ignored when those are not given.
    #[arg(value_name = "PACK")]
    pub pack: Option<PathBuf>,

    /// Source path: take plans from this directory and apply them into --root.
    #[arg(long, value_name = "DIR", conflicts_with = "pack")]
    pub plans: Option<PathBuf>,

    /// Plan names to launch when using --plans (positional after the flag).
    #[arg(value_name = "PLAN", requires = "plans")]
    pub plan_targets: Vec<String>,

    /// Resolve a pack by name from `pack_dirs` (default: /var/lib/wright/packs).
    #[arg(long, value_name = "NAME", conflicts_with_all = ["pack", "plans"])]
    pub profile: Option<String>,

    /// Print install order and overlay/config actions without writing anything.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Reinstall parts that already exist in the target root.
    #[arg(long, short = 'f')]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct PackArgs {
    /// Directory containing `pack.toml`, `parts/`, and an optional `overlay/`.
    /// Or, when the first positional is `inspect`, the path of a pack file to read.
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// When set, write the pack archive to this path. Defaults to
    /// `<name>-<version>.wright.pack.tar` in the current directory.
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Inspect a pack file instead of building one.
    #[arg(long)]
    pub inspect: bool,
}
