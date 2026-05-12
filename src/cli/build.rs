use clap::Args;
use std::path::PathBuf;

pub const BUILD_AFTER_HELP: &str = "\
Examples:
  wright build zlib
  wright build zlib --rebuild --clean
  wright build freetype --mvp --stage=configure
  wright build freetype --until-stage=staging
  wright resolve openssl --rdeps | wright build
  echo -e 'curl\nwget' | wright build --rebuild

Resume is automatic via forge file-system checkpoints keyed by plan content hash.";

pub const LAUNCH_AFTER_HELP: &str = "\
Examples:
  wright launch --root /mnt/new --folio ./folios/core.toml
  wright launch --root /mnt/new --plans ./plans bash coreutils glibc
  wright launch --root /mnt/new --plans ./plans @core
  wright launch --root /mnt/new @base @maintenance @desktop

Launch fills a target root from a folio manifest or from explicit plan names.
When --folio is given, the manifest names the plans to resolve, forge, and
deploy.  When positional arguments are given, they are plan names or folio
names prefixed with '@'.  If --plans is omitted, the default plans directory
from wright.toml is used.  Re-running launch on the same root converges drift
rather than erroring.";

#[derive(Args, Debug, Clone)]
#[command(after_help = BUILD_AFTER_HELP)]
pub struct BuildArgs {
    /// Paths to plan directories or part names
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Run only the specified lifecycle stages, in pipeline order; may be repeated.
    /// Skips fetch/verify/extract — requires a previous full forge.
    /// Example: --stage=check --stage=staging
    #[arg(long, conflicts_with = "until_stage")]
    pub stage: Vec<String>,

    /// Force re-run of a specific stage even if its checkpoint is valid.
    /// Other stages still obey normal checkpoint rules.
    /// Example: --force-stage=check
    #[arg(long)]
    pub force_stage: Vec<String>,

    /// Run a normal forge pipeline and stop after the specified lifecycle stage.
    /// Unlike `--stage`, this still runs all prior stages in order.
    #[arg(long, conflicts_with = "stage")]
    pub until_stage: Option<String>,

    /// Skip the lifecycle `check` stage during a normal full forge.
    /// Unlike `--stage`, this still runs the full pipeline (including fetch/verify/extract).
    #[arg(long, conflicts_with = "stage")]
    pub skip_check: bool,

    /// Clear the forge cache, source tree, and working directory before
    /// starting. Without --clean, work/ is preserved for incremental forges
    /// when the forge key is unchanged. Composable with --rebuild.
    #[arg(long, short = 'c')]
    pub clean: bool,

    /// Reforge from scratch: bypass stage checkpoints and re-run all
    /// lifecycle stages. Use this when you have modified a plan's forge
    /// script or dependencies and need a clean reforge.
    #[arg(long, short = 'R')]
    pub rebuild: bool,

    /// Forge using the MVP dependency set from mvp.toml without
    /// requiring a dependency cycle to trigger it
    #[arg(long)]
    pub mvp: bool,

    /// Download sources only; do not forge
    #[arg(long, conflicts_with = "checksum")]
    pub fetch: bool,

    /// Compute and update SHA256 checksums in plan.toml
    #[arg(long, conflicts_with = "fetch")]
    pub checksum: bool,
}

#[derive(Args)]
pub struct LintArgs {
    /// Plan names or paths to validate (all plans if omitted)
    pub targets: Vec<String>,
    /// Recurse into subdirectories
    #[arg(long, short = 'r')]
    pub recursive: bool,
    /// Verify deployed part file integrity (SHA-256 checksums)
    #[arg(long)]
    pub verify: bool,
}

#[derive(Args, Debug)]
#[command(after_help = LAUNCH_AFTER_HELP)]
pub struct LaunchArgs {
    /// Path to a `folio.toml` file. Mutually exclusive with --plans.
    #[arg(long, value_name = "FILE", conflicts_with = "plans")]
    pub folio: Option<PathBuf>,

    /// Source path: take plans from this directory and apply them into --root.
    /// Positional arguments are plan names, or folio names prefixed with '@'.
    #[arg(long, value_name = "DIR", conflicts_with = "folio")]
    pub plans: Option<PathBuf>,

    /// Plan or folio names to launch when using --plans.
    /// Names starting with '@' are resolved as folios under the plans directory.
    #[arg(value_name = "TARGET")]
    pub plan_targets: Vec<String>,

    /// Print deploy order and config actions without writing anything.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Reforge and redeploy parts that already exist in the target root.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}
