use clap::Args;
use std::path::PathBuf;

use crate::cli::common::{DomainArg, MatchPolicyArg};
#[cfg(with_handlers)]
use crate::cli::common::Context;
#[cfg(with_handlers)]
use crate::error::{Result, WrightError};
#[cfg(with_handlers)]
use crate::operations::install::{InstallRequest, execute_install};
#[cfg(with_handlers)]
use crate::resolve::{DepDomain, DependentsMode, MatchPolicy};
#[cfg(with_handlers)]
use crate::util::stdin::collect_stdin_args;

const WRIGHT_INSTALL_AFTER_HELP_FULL: &str = "\
Examples:
  wright install zlib
  wright install zlib openssl
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

    /// Expand dependencies.
    /// `link` follows ABI-sensitive link dependencies.
    /// `runtime` follows runtime dependencies.
    /// `build` follows build dependencies.
    /// `all` follows all dependencies.
    #[arg(
        short = 'd',
        long = "deps",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "all"
    )]
    pub deps: Option<DomainArg>,

    /// Expand reverse dependents (rdeps) for deployed parts.
    /// `link` follows ABI-sensitive link dependents.
    /// `runtime` follows runtime dependents.
    /// `build` follows build dependents.
    /// `all` follows all dependents.
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

    /// Force a clean reforge and redeploy even if matching parts already exist
    #[arg(long, short = 'f')]
    pub force: bool,

    /// Preview what would be forged and deployed without making any changes
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Alternate root directory for file operations
    #[arg(long)]
    pub root: Option<PathBuf>,
}

#[cfg(with_handlers)]
fn map_resolve_domain(d: DomainArg) -> DependentsMode {
    match d {
        DomainArg::Link => DependentsMode::Link,
        DomainArg::Runtime => DependentsMode::Runtime,
        DomainArg::Forge => DependentsMode::Forge,
        DomainArg::All => DependentsMode::All,
    }
}

#[cfg(with_handlers)]
fn map_match_policy(m: MatchPolicyArg) -> MatchPolicy {
    match m {
        MatchPolicyArg::All => MatchPolicy::All,
        MatchPolicyArg::Missing => MatchPolicy::Missing,
        MatchPolicyArg::Outdated => MatchPolicy::Outdated,
        MatchPolicyArg::Installed => MatchPolicy::Installed,
    }
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
            "no targets specified (pass plan names, group names prefixed with '@', or paths as arguments or via stdin)".into(),
        ));
    }

    if args.dry_run {
        println!("Apply plan (dry-run):");
        println!("  targets: {}", targets.join(", "));
        return Ok(());
    }

    let dep_domain = if args.deps.is_none() && args.rdeps.is_none() {
        DepDomain::ALL
    } else {
        let mut domain = DepDomain::empty();
        if let Some(d) = args.deps {
            domain.insert(DepDomain::from_dependents_mode(map_resolve_domain(d)));
        }
        if let Some(d) = args.rdeps {
            domain.insert(DepDomain::from_dependents_mode(map_resolve_domain(d)));
        }
        domain
    };

    execute_install(InstallRequest {
        targets,
        dep_domain,
        match_policies: args.match_policies.into_iter().map(map_match_policy).collect(),
        depth: args.depth,
        force: args.force,
        config: ctx.config,
        db_path: &ctx.db_path,
        root_dir: &ctx.root_dir,
        verbose: ctx.verbose,
        quiet: ctx.quiet,
        part_store: &part_store,
        forge_opts: None,
    })
    .await
}
