use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::Result;

pub const BUILD_AFTER_HELP: &str = "\
Examples:
  wright build zlib
  wright build zlib --force --fresh
  wright build zlib --no-cache
  wright build zlib --no-cache --dry-run
  wright build freetype --mvp --stage=configure
  wright build freetype --until-stage=staging
  wright resolve openssl --rdeps | wright build
  echo -e 'curl\nwget' | wright build --force

Resume is automatic via forge file-system checkpoints keyed by plan content hash.";

#[derive(Args, Debug, Clone)]
#[command(after_help = BUILD_AFTER_HELP)]
pub struct BuildArgs {
    /// Paths to plan directories or part names
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Run only the specified pipeline stages, in pipeline order; may be repeated.
    /// Skips fetch/verify/extract — requires a previous full forge.
    /// Example: --stage=check --stage=staging
    #[arg(long, conflicts_with = "until_stage")]
    pub stage: Vec<String>,

    /// Force re-run of a specific stage even if its checkpoint is valid.
    /// Other stages still obey normal checkpoint rules.
    /// Example: --force-stage=check
    #[arg(long)]
    pub force_stage: Vec<String>,

    /// Run a normal forge pipeline and stop after the specified pipeline stage.
    /// Unlike `--stage`, this still runs all prior stages in order.
    #[arg(long, conflicts_with = "stage")]
    pub until_stage: Option<String>,

    /// Skip the pipeline `check` stage during a normal full forge.
    /// Unlike `--stage`, this still runs the full pipeline (including fetch/verify/extract).
    #[arg(long, conflicts_with = "stage")]
    pub skip_check: bool,

    /// Wipe the forge workspace — source tree, intermediates, and stage
    /// checkpoints — before building, so the forge starts from scratch.
    /// Without --fresh, work/ is preserved for incremental forges when the
    /// forge key is unchanged. Composable with --force.
    #[arg(long, short = 'c', alias = "clean")]
    pub fresh: bool,

    /// Reforge from scratch: bypass stage checkpoints and re-run all
    /// pipeline stages. Use this when you have modified a plan's forge
    /// script or dependencies and need a clean rebuild.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Cold build: transactionally purge the source cache and the CAS store
    /// (verified pinned http inputs are kept), then rebuild from freshly
    /// fetched sources, bypassing every cached artifact (source cache, charge
    /// marker, stage checkpoints, staging cache, layers). Stronger than
    /// `--fresh`, which clears only the per-plan forge workspace. Mutually
    /// exclusive with `--stage` and `--force-stage`, which depend on existing
    /// checkpoints.
    #[arg(long, conflicts_with_all = ["stage", "force_stage"])]
    pub no_cache: bool,

    /// Preview what a cold build would purge and build, then exit without
    /// changing anything
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Forge using the MVP dependency set from mvp.toml without
    /// requiring a dependency cycle to trigger it
    #[arg(long)]
    pub mvp: bool,

    /// Download sources only; do not forge
    #[arg(long, conflicts_with = "checksum")]
    pub fetch: bool,

    /// Seal forged staging trees into part archives in parts_dir after the
    /// forge completes, without deploying. Restores the build → seal →
    /// merge workflow for preparing archives ahead of `wright merge`.
    #[arg(long, conflicts_with_all = ["fetch", "checksum", "until_stage"])]
    pub seal: bool,

    /// Compute and update SHA256 checksums in plan.toml
    #[arg(long, conflicts_with = "fetch")]
    pub checksum: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: BuildArgs, ctx: &Context<'_>) -> Result<()> {
    let request = crate::operations::build::BuildRequest {
        targets: args.targets,
        stages: args.stage,
        force_stages: args.force_stage,
        until_stage: args.until_stage,
        skip_check: args.skip_check,
        clean: args.fresh,
        force: args.force,
        no_cache: args.no_cache,
        mvp: args.mvp,
        fetch_only: args.fetch,
        seal: args.seal,
        checksum: args.checksum,
        dry_run: args.dry_run,
    };
    crate::operations::build::execute_build(
        request,
        ctx.config,
        &ctx.db_path,
        ctx.verbose,
        ctx.quiet,
    )
    .await
}
