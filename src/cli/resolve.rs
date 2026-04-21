use clap::{Parser, ValueEnum};

pub const RESOLVE_AFTER_HELP: &str = "\
Examples:
  wright resolve zlib --deps
  wright resolve zlib --deps --match=outdated
  wright resolve openssl --rdeps
  wright resolve glibc --rdeps=all --depth=0
  wright resolve zlib --deps=link --match=outdated

Pipe into wright build:
  wright resolve openssl --rdeps | wright build";

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum MatchPolicyArg {
    /// Include plans that are not currently installed.
    Missing,
    /// Include plans whose version/release differs from the installed one.
    Outdated,
    /// Include plans that are already installed and match the plan definition.
    Installed,
    /// Include all plans.
    All,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DomainArg {
    /// Follow only ABI-sensitive link relationships.
    Link,
    /// Follow only runtime relationships.
    Runtime,
    /// Follow only build-time relationships.
    Build,
    /// Follow all relationships (link + runtime + build).
    All,
}

#[derive(Parser, Debug, Clone)]
pub struct ResolveArgs {
    /// Paths to plan directories, part names, or @assemblies
    pub targets: Vec<String>,

    /// Exclude the listed target plans themselves from the output
    #[arg(short = 'x', long = "exclude-targets")]
    pub exclude_targets: bool,

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

    /// Expand reverse dependents (rdeps) for installed parts.
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

    /// Match policy for filtering based on installation state.
    /// Can be specified multiple times.
    #[arg(long = "match", alias = "match-policies", value_enum, default_values = &["all"])]
    pub match_policies: Vec<MatchPolicyArg>,

    /// Maximum expansion depth. `0` means unlimited.
    /// If omitted, reverse-dependent expansion defaults to depth 1;
    /// other expansions default to unlimited.
    #[arg(long)]
    pub depth: Option<usize>,

    /// Show a visual dependency tree from hold-tree plan.toml files.
    /// This is a static analysis mode — it does not read the installed
    /// part database.
    #[arg(long, short = 't', conflicts_with_all = ["deps", "rdeps", "match_policies", "exclude_targets"])]
    pub tree: bool,
}
