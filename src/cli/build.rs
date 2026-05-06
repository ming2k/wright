use clap::Parser;

pub const BUILD_AFTER_HELP: &str = "\
Examples:
  wright build zlib
  wright build zlib --force --clean
  wright build freetype --mvp --stage=configure
  wright build freetype --until-stage=staging
  wright resolve openssl --rdeps | wright build
  echo -e 'curl\\nwget' | wright build --force

Resume: rerunning the same command resumes any incomplete run automatically.
Use --fresh to discard prior state and start over.";

#[derive(Parser, Debug, Clone)]
pub struct BuildArgs {
    /// Paths to plan directories or part names
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
    /// starting. Without --clean, work/ is preserved for incremental builds
    /// when the build key is unchanged. Composable with --force.
    #[arg(long, short = 'c')]
    pub clean: bool,

    /// Force rebuild: bypass the build cache, re-run all lifecycle
    /// stages, and overwrite existing output parts
    #[arg(long, short)]
    pub force: bool,

    /// Discard any prior workflow state for these inputs and start from
    /// scratch. By default, rerunning the same command resumes the prior
    /// run (skipping completed steps).
    #[arg(long)]
    pub fresh: bool,

    /// Build using the MVP dependency set from mvp.toml without
    /// requiring a dependency cycle to trigger it
    #[arg(long)]
    pub mvp: bool,

    /// Download sources only; do not build
    #[arg(long, conflicts_with = "checksum")]
    pub fetch: bool,

    /// Compute and update SHA256 checksums in plan.toml
    #[arg(long, conflicts_with = "fetch")]
    pub checksum: bool,
}
