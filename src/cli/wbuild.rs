use clap::{Parser, ValueEnum};

pub const WBUILD_RUN_AFTER_HELP: &str = "\
Examples:
  wright build zlib
  wright build zlib --force --clean
  wright build freetype --mvp --stage=configure
  wright resolve openssl --self --dependents | wright build
  echo -e 'curl\\nwget' | wright build --force";
pub const WBUILD_RESOLVE_AFTER_HELP: &str = "\
Examples:
  wright resolve zlib --self --deps
  wright resolve zlib --self --deps=sync
  wright resolve openssl --self --dependents
  wright resolve glibc --self --dependents=all --depth=0
  wright resolve zlib --self --deps=all

Pipe into wright build:
  wright resolve openssl --self --dependents | wright build";

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
pub struct RunArgs {
    /// Paths to plan directories, part names, or @assemblies
    pub targets: Vec<String>,

    /// Run only the specified lifecycle stages, in pipeline order; may be repeated.
    /// Skips fetch/verify/extract — requires a previous full build.
    /// Example: --stage=check --stage=staging --stage=fabricate
    #[arg(long)]
    pub stage: Vec<String>,

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

    /// Max number of concurrent dockyards. Only parts with no
    /// direct or indirect dependency relationship run simultaneously;
    /// the scheduler enforces ordering automatically.
    /// 0 = auto-detect CPU count.
    #[arg(short = 'w', long, default_value = "0")]
    pub dockyards: usize,

    /// Build using the MVP dependency set from mvp.toml without
    /// requiring a dependency cycle to trigger it
    #[arg(long)]
    pub mvp: bool,

    /// Print produced archive paths to stdout after a successful build
    #[arg(long)]
    pub print_archives: bool,

    /// Remove all saved build sessions and exit
    #[arg(long)]
    pub clear_sessions: bool,

    /// Download sources only; do not build
    #[arg(long, conflicts_with_all = ["checksum", "lint"])]
    pub fetch: bool,

    /// Compute and update SHA256 checksums in plan.toml
    #[arg(long, conflicts_with_all = ["fetch", "lint"])]
    pub checksum: bool,

    /// Validate plan.toml files for syntax and logic errors
    #[arg(long, conflicts_with_all = ["fetch", "checksum"])]
    pub lint: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct ResolveArgs {
    /// Paths to plan directories, part names, or @assemblies
    pub targets: Vec<String>,

    /// Include the listed parts themselves in the output
    #[arg(short = 's', long = "self")]
    pub include_self: bool,

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
    #[arg(long, short = 't', conflicts_with_all = ["deps", "dependents", "include_self"])]
    pub tree: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct PruneArgs {
    /// Delete archives that are present on disk but not registered in the inventory DB
    #[arg(long)]
    pub untracked: bool,

    /// Keep only the latest tracked archive per part name, while preserving installed versions
    #[arg(long)]
    pub latest: bool,

    /// Apply deletions. Without this flag, only prints what would change
    #[arg(long)]
    pub apply: bool,
}
