use clap::Args;
use std::path::PathBuf;

#[cfg(with_handlers)]
use crate::cli::common::Context;
use crate::cli::common::{DomainArg, MatchPolicyArg};
#[cfg(with_handlers)]
use crate::cli::common::{map_domain, map_match_policy};
#[cfg(with_handlers)]
use crate::error::{Result, WrightError};
#[cfg(with_handlers)]
use crate::operations::install::{InstallRequest, execute_install};
#[cfg(with_handlers)]
use crate::resolve::DepDomain;
#[cfg(with_handlers)]
use crate::util::stdin::collect_stdin_args;

const WRIGHT_INSTALL_AFTER_HELP_FULL: &str = "\
Examples:
  wright install zlib
  wright install zlib openssl
  wright install zlib --fresh
  wright install ./plans/bash
  wright install @core
  wright install gcc --match=all";

#[derive(Args)]
#[command(
    long_about = "Install plans to the local system.\n\nTargets may be plan names, plan directories, or folio names prefixed with '@'. Wright is the high-level source-first combo command: it resolves requested targets, automatically pulls in all dependencies (build, link, and runtime) that are missing or outdated under the selected match policy, forges what is needed in dependency waves, seals outputs, and merges each completed wave onto the live system. Use it for natural plan-driven install and upgrade workflows.",
    after_help = WRIGHT_INSTALL_AFTER_HELP_FULL
)]
pub struct InstallArgs {
    /// Plan names, plan directories, or folio names prefixed with '@'
    #[arg(value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Expand dependencies: `link` follows ABI-sensitive link dependencies,
    /// `runtime` runtime dependencies, `build` build dependencies, `all` all of
    /// them. Bare `--deps` means `all`; when omitted, all domains are followed.
    #[arg(
        short = 'd',
        long = "deps",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "all"
    )]
    pub deps: Option<DomainArg>,

    /// Additionally rebuild deployed parts that depend on the targets:
    /// `link` follows ABI-sensitive link dependents, `runtime` runtime
    /// dependents, `build` build dependents, `all` all of them. Bare `--rdeps`
    /// means `link`; when omitted, no reverse expansion happens.
    #[arg(
        short = 'r',
        long = "rdeps",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "link"
    )]
    pub rdeps: Option<DomainArg>,

    /// Match policy for filtering based on deployment state.
    /// Can be specified multiple times. If omitted, `install` defaults to
    /// `outdated`, so missing and changed dependencies are added
    /// automatically while already-converged ones are skipped.
    #[arg(long = "match", alias = "match-policies", value_enum)]
    pub match_policies: Vec<MatchPolicyArg>,

    /// Maximum expansion depth. `0` means unlimited.
    #[arg(long)]
    pub depth: Option<usize>,

    /// Force a from-scratch rebuild and redeploy even if matching parts
    /// already exist. Implies `--fresh`.
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Wipe the forge workspace (source and working trees) before building,
    /// so parts that need building are forged from scratch. Unlike `--force`,
    /// this does not redeploy parts that are already up to date. Composable
    /// with `--force`.
    #[arg(long, short = 'c', alias = "clean")]
    pub fresh: bool,

    /// Preview what would be forged and deployed without making any changes
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
pub async fn run(args: InstallArgs, ctx: &Context<'_>) -> Result<()> {
    let (part_store, _lock) = ctx.ensure_lock_and_part_store()?;

    let targets = collect_stdin_args(args.targets)?;
    if targets.is_empty() {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            return Err(WrightError::ForgeError(
                "no targets received from stdin; did the resolve succeed?".into(),
            ));
        }
        return Err(WrightError::ForgeError(
            "no targets specified (pass plan names, folio names prefixed with '@', or paths as arguments or via stdin)".into(),
        ));
    }

    // Forward deps and reverse dependents are independent axes: `--deps`
    // restricts which forward relationships are followed (default `all`),
    // while `--rdeps` additionally rebuilds deployed parts that depend on
    // the targets (bare `--rdeps` means `link`; omitted means none).
    let deps = args.deps.map(map_domain).unwrap_or(DepDomain::ALL);
    let rdeps = args.rdeps.map(map_domain).unwrap_or_else(DepDomain::empty);

    execute_install(InstallRequest {
        targets,
        deps,
        rdeps,
        match_policies: args
            .match_policies
            .into_iter()
            .map(map_match_policy)
            .collect(),
        depth: args.depth,
        force: args.force,
        clean: args.fresh,
        config: ctx.config,
        db_path: &ctx.db_path,
        root_dir: &ctx.root_dir,
        verbose: ctx.verbose,
        quiet: ctx.quiet,
        part_store: &part_store,
        build_opts: None,
        run_hooks: true,
        dry_run: args.dry_run,
    })
    .await
}
