use clap::{Parser, ValueEnum};

pub const RESOLVE_AFTER_HELP: &str = "\
Examples:
  wright resolve zlib --deps
  wright resolve zlib --deps --rebuild=outdated
  wright resolve openssl --rdeps
  wright resolve glibc --rdeps=all --depth=0
  wright resolve zlib --deps=link --rebuild=outdated

Pipe into wright build:
  wright resolve openssl --rdeps | wright build";

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum RebuildPolicyArg {
    /// Include all traversed plans, regardless of installed state.
    All,
    /// Only include plans that are not currently installed.
    Missing,
    /// Only include plans that are missing, or whose version/release differs from the installed one.
    Outdated,
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

    /// Expand upstream dependencies.
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

    /// Expand downstream reverse dependencies (rdeps) for installed parts.
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

    /// Rebuild policy based on current installation state.
    /// `missing` keeps only absent parts.
    /// `outdated` keeps parts that are missing or differ from the plan.
    /// `all` performs no filtering, keeping everything.
    #[arg(long, value_enum, default_value = "all")]
    pub rebuild: RebuildPolicyArg,

    /// Maximum expansion depth. `0` means unlimited.
    /// If omitted, reverse-dependent expansion defaults to depth 1;
    /// other expansions default to unlimited.
    #[arg(long)]
    pub depth: Option<usize>,

    /// Show a visual dependency tree from hold-tree plan.toml files.
    /// This is a static analysis mode — it does not read the installed
    /// part database.
    #[arg(long, short = 't', conflicts_with_all = ["deps", "rdeps", "rebuild", "exclude_targets"])]
    pub tree: bool,
}
