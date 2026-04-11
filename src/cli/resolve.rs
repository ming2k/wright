use clap::{Parser, ValueEnum};

pub const RESOLVE_AFTER_HELP: &str = "\
Examples:
  wright resolve zlib --include-targets --deps
  wright resolve zlib --include-targets --deps=sync
  wright resolve openssl --include-targets --dependents
  wright resolve glibc --include-targets --dependents=all --depth=0
  wright resolve zlib --include-targets --deps=all

Pipe into wright build:
  wright resolve openssl --include-targets --dependents | wright build";

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DepsMode {
    /// Do not auto-expand upstream dependencies.
    None,
    /// Add missing upstream dependencies from the hold tree.
    Missing,
    /// Add upstream dependencies whose installed version differs from the plan.
    Sync,
    /// Add all upstream dependencies, even when already installed at the same version.
    All,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DependentsModeArg {
    /// Rebuild only direct/transitive link dependents.
    Link,
    /// Rebuild link, runtime, and build dependents.
    All,
}

#[derive(Parser, Debug, Clone)]
pub struct ResolveArgs {
    /// Paths to plan directories, part names, or @assemblies
    pub targets: Vec<String>,

    /// Include the listed target plans themselves in the output
    #[arg(short = 's', long = "include-targets")]
    pub include_targets: bool,

    /// Expand upstream dependencies.
    /// `missing` adds only absent dependencies.
    /// `sync` also rebuilds installed dependencies whose epoch/version/release
    /// differs from the current plan.
    /// `all` rebuilds all upstream dependencies regardless of installed state.
    #[arg(
        short = 'd',
        long = "deps",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "missing"
    )]
    pub deps: Option<DepsMode>,

    /// Expand downstream dependents (installed parts only).
    /// `link` follows ABI-sensitive link dependents.
    /// `all` also follows runtime and build dependents.
    #[arg(
        long = "dependents",
        value_enum,
        num_args = 0..=1,
        default_missing_value = "link"
    )]
    pub dependents: Option<DependentsModeArg>,

    /// Maximum expansion depth. `0` means unlimited.
    /// If omitted, reverse-dependent expansion defaults to depth 1;
    /// other expansions default to unlimited.
    #[arg(long)]
    pub depth: Option<usize>,

    /// Show a visual dependency tree from hold-tree plan.toml files.
    /// This is a static analysis mode — it does not read the installed
    /// part database.
    #[arg(long, short = 't', conflicts_with_all = ["deps", "dependents", "include_targets"])]
    pub tree: bool,
}
