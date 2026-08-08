use clap::Args;

#[cfg(with_handlers)]
use crate::cli::common::{Context, map_domain, map_match_policy};
use crate::cli::common::{DomainArg, MatchPolicyArg};
#[cfg(with_handlers)]
use crate::error::Result;
#[cfg(with_handlers)]
use crate::operations::resolve::ResolveRequest;
#[cfg(with_handlers)]
use crate::resolve::{DepDomain, MatchPolicy};

const WRIGHT_RESOLVE_AFTER_HELP: &str = "\
Examples:
  wright resolve hello
  wright resolve hello --deps --match=outdated
  wright resolve openssl --rdeps=link --depth=0
  wright resolve hello --deps --tree";

#[derive(Args)]
#[command(
    long_about = "Compute dependency execution graph for targets (Step 1 of Delivery pipeline).",
    after_help = WRIGHT_RESOLVE_AFTER_HELP
)]
pub struct ResolveArgs {
    /// Target plan names or plan directory paths to resolve
    #[arg(required = true, value_name = "TARGET")]
    pub targets: Vec<String>,

    /// Expand dependencies by relationship type
    #[arg(
        short = 'd',
        long,
        value_enum,
        num_args = 0..=1,
        default_missing_value = "all"
    )]
    pub deps: Option<DomainArg>,

    /// Expand reverse dependents by relationship type
    #[arg(
        short = 'r',
        long,
        value_enum,
        num_args = 0..=1,
        default_missing_value = "link"
    )]
    pub rdeps: Option<DomainArg>,

    /// Filter plans by installed state; may be repeated
    #[arg(long = "match", alias = "match-policies", value_enum)]
    pub match_policies: Vec<MatchPolicyArg>,

    /// Maximum traversal depth; `0` means unlimited
    #[arg(long)]
    pub depth: Option<usize>,

    /// Display the result as a dependency forest
    #[arg(long, short)]
    pub tree: bool,
}

#[cfg(with_handlers)]
pub async fn run(args: ResolveArgs, ctx: &Context<'_>) -> Result<()> {
    let mut deps = args.deps.map(map_domain).unwrap_or_else(DepDomain::empty);
    let rdeps = args.rdeps.map(map_domain).unwrap_or_else(DepDomain::empty);
    if args.tree && deps.is_empty() && rdeps.is_empty() {
        deps = DepDomain::ALL;
    }

    let match_policies = if args.match_policies.is_empty() {
        vec![MatchPolicy::All]
    } else {
        args.match_policies
            .into_iter()
            .map(map_match_policy)
            .collect()
    };

    crate::operations::resolve::execute_resolve(
        ResolveRequest {
            targets: args.targets,
            deps,
            rdeps,
            match_policies,
            depth: args.depth,
            tree: args.tree,
        },
        ctx.config,
    )
    .await
}
