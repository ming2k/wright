use clap::Parser;

pub const BUILD_AFTER_HELP: &str = "\
Examples:
  wright build zlib
  wright build zlib --force --clean
  wright build freetype --mvp --stage=configure
  wright build freetype --until-stage=staging
  wright resolve openssl --dependents | wright build
  echo -e 'curl\\nwget' | wright build --force";

#[derive(Parser, Debug, Clone)]
pub struct BuildArgs {
    /// Paths to plan directories, part names, or @assemblies
    pub targets: Vec<String>,

    /// Run only the specified lifecycle stages, in pipeline order; may be repeated.
    /// Skips fetch/verify/extract — requires a previous full build.
    /// Example: --stage=check --stage=staging --stage=fabricate
    #[arg(long, conflicts_with = "until_stage")]
    pub stage: Vec<String>,

    /// Run a normal build pipeline and stop after the specified lifecycle stage.
    /// Unlike `--stage`, this still runs all prior stages in order.
    #[arg(long, conflicts_with = "stage")]
    pub until_stage: Option<String>,

    /// Skip the lifecycle `check` stage during a normal full build.
    /// Unlike `--stage`, this still runs the full pipeline (including fetch/verify/extract).
    #[arg(long, conflicts_with = "stage")]
    pub skip_check: bool,

    /// Clear the build cache, source tree, and working directory before
    /// starting. Without --clean, src/ is preserved for incremental builds
    /// when the build key is unchanged. Composable with --force.
    #[arg(long, short = 'c')]
    pub clean: bool,

    /// Force rebuild: overwrite existing archive and bypass the build cache
    #[arg(long, short)]
    pub force: bool,

    /// Resume a previous build session: skip parts that were already
    /// successfully built and installed. Optionally pass a session hash
    /// (printed on failure); without a hash, auto-detects from the
    /// current build set.
    #[arg(long, short, num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Build using the MVP dependency set from mvp.toml without
    /// requiring a dependency cycle to trigger it
    #[arg(long)]
    pub mvp: bool,

    /// Print produced archive paths to stdout after a successful build.
    /// Human-readable logs continue to go to stderr so this remains pipe-safe.
    #[arg(long)]
    pub print_parts: bool,

    /// Remove all saved build sessions and exit
    #[arg(long)]
    pub clear_sessions: bool,

    /// Download sources only; do not build
    #[arg(long, conflicts_with = "checksum")]
    pub fetch: bool,

    /// Compute and update SHA256 checksums in plan.toml
    #[arg(long, conflicts_with = "fetch")]
    pub checksum: bool,
}
