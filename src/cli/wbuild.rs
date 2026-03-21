use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

const WBUILD_AFTER_HELP: &str = "\
Workflows:
  Build a part:            wbuild run zlib
  Rebuild part only:       wbuild run zlib --self
  Cascade to dependents:   wbuild run openssl --self --dependents
  Full reverse cascade:    wbuild run glibc --self --dependents=all --depth=0
  Validate plans:          wbuild check ./plans/zlib

Targets may be plan names, plan directories, or `@assembly` references.";
const WBUILD_RUN_AFTER_HELP: &str = "\
Examples:
  wbuild run zlib
  wbuild run zlib --self
  wbuild run zlib --deps
  wbuild run zlib --deps=sync -i
  wbuild run openssl --self --dependents
  wbuild run glibc --self --dependents=all --depth=0
  wbuild run freetype --mvp --stage=configure
  wbuild run zlib --deps=all";
const WBUILD_CHECK_AFTER_HELP: &str = "\
Examples:
  wbuild check zlib
  wbuild check ./plans/zlib";
const WBUILD_FETCH_AFTER_HELP: &str = "\
Examples:
  wbuild fetch zlib
  wbuild fetch ./plans/zlib";
const WBUILD_DEPS_AFTER_HELP: &str = "\
Examples:
  wbuild deps zlib
  wbuild deps zlib --depth=2

This command reads dependency declarations from `plan.toml` files in the hold tree.
It does not inspect the installed part database.";
const WBUILD_CHECKSUM_AFTER_HELP: &str = "\
Examples:
  wbuild checksum zlib
  wbuild checksum ./plans/zlib";

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
pub enum DependentsMode {
    /// Rebuild only direct/transitive link dependents.
    Link,
    /// Rebuild link, runtime, and build dependents.
    All,
}

#[derive(Parser)]
#[command(
    name = "wbuild",
    about = "Build and validate Wright part plans",
    long_about = "Build and validate Wright part plans.\n\nUse `wbuild` for part construction: resolve build graphs, fetch sources, run lifecycle stages, and optionally install finished archives.",
    after_help = WBUILD_AFTER_HELP,
    version,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Alternate root directory for file operations
    #[arg(long, global = true, help_heading = "Global Options")]
    pub root: Option<PathBuf>,

    /// Path to config file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub config: Option<PathBuf>,

    /// Path to database file
    #[arg(long, global = true, help_heading = "Global Options")]
    pub db: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(long, short = 'v', global = true, action = ArgAction::Count, help_heading = "Global Options")]
    pub verbose: u8,

    /// Reduce log output (show warnings/errors only)
    #[arg(long, global = true, help_heading = "Global Options")]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Build parts from plans (default operation)
    #[command(
        long_about = "Build parts from plans.\n\nTargets may be plan names, plan directories, or `@assembly` references. By default, `wbuild run` builds only the listed targets. Use `--deps=<MODE>` to expand upstream dependencies explicitly.",
        after_help = WBUILD_RUN_AFTER_HELP
    )]
    Run {
        /// Paths to plan directories, part names, or @assemblies
        targets: Vec<String>,

        /// Run only the specified lifecycle stages, in pipeline order; may be repeated.
        /// Skips fetch/verify/extract — requires a previous full build.
        /// Example: --stage=check --stage=staging --stage=fabricate
        #[arg(long)]
        stage: Vec<String>,

        /// Skip the lifecycle `check` stage during a normal full build.
        /// Unlike `--stage`, this still runs the full pipeline (including fetch/verify/extract).
        #[arg(long, conflicts_with = "stage")]
        skip_check: bool,

        /// Clear the build cache, source tree, and working directory before
        /// starting. Without --clean, src/ is preserved for incremental builds
        /// when the build key is unchanged. Composable with --force.
        #[arg(long, short = 'c')]
        clean: bool,

        /// Force rebuild: overwrite existing archive and bypass the build cache
        #[arg(long, short)]
        force: bool,

        /// Max number of concurrent dockyards. Only parts with no
        /// direct or indirect dependency relationship run simultaneously;
        /// the scheduler enforces ordering automatically.
        /// 0 = auto-detect CPU count.
        #[arg(short = 'w', long, default_value = "0")]
        dockyards: usize,

        /// Automatically install each part after a successful build
        #[arg(short = 'i', long)]
        install: bool,

        /// Maximum expansion depth for dependency cascade operations.
        /// `0` means unlimited. If omitted, reverse-dependent expansion defaults
        /// to depth 1; other expansions default to unlimited.
        #[arg(long)]
        depth: Option<usize>,

        /// Include the listed parts themselves in the build
        #[arg(short = 's', long = "self")]
        include_self: bool,

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
        deps: Option<DepsMode>,

        /// Expand downstream dependents.
        /// `link` follows ABI-sensitive link dependents.
        /// `all` also follows runtime and build dependents.
        #[arg(
            long = "dependents",
            value_enum,
            num_args = 0..=1,
            default_missing_value = "link"
        )]
        dependents: Option<DependentsMode>,

        /// Build using the MVP dependency set from inline [mvp.dependencies]
        /// or sibling mvp.toml without requiring a dependency cycle to trigger it
        #[arg(long)]
        mvp: bool,
    },
    /// Validate plan.toml files for syntax and logic errors
    #[command(
        long_about = "Validate `plan.toml` files for syntax and logic errors without building parts.",
        after_help = WBUILD_CHECK_AFTER_HELP
    )]
    Check {
        /// Plans to check
        targets: Vec<String>,
    },
    /// Download sources for plans without building
    #[command(
        long_about = "Fetch plan sources and verify them without continuing into the full build pipeline.",
        after_help = WBUILD_FETCH_AFTER_HELP
    )]
    Fetch {
        /// Plans to fetch
        targets: Vec<String>,
    },
    /// Analyze plan dependency relationships from hold-tree plan.toml files
    #[command(
        long_about = "Analyze plan dependency relationships from `plan.toml` files in the hold tree.\n\nThis command shows the declared build/link/runtime dependency graph for a plan. It does not read the installed part database or `.PARTINFO` metadata.",
        after_help = WBUILD_DEPS_AFTER_HELP
    )]
    Deps {
        /// Target plan name
        target: String,

        /// Maximum depth to display
        #[arg(long, short, default_value = "0")]
        depth: usize,
    },
    /// Compute and update SHA256 checksums in plan.toml
    #[command(
        long_about = "Download sources as needed, compute SHA-256 checksums, and update the corresponding `plan.toml` entries.",
        after_help = WBUILD_CHECKSUM_AFTER_HELP
    )]
    Checksum {
        /// Plans to checksum
        targets: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands, DependentsMode, DepsMode};
    use clap::Parser;

    #[test]
    fn parse_run_defaults_to_no_dependency_expansion() {
        let cli = Cli::try_parse_from(["wbuild", "run", "zlib"]).unwrap();
        match cli.command {
            Commands::Run { deps, .. } => assert_eq!(deps, None),
            _ => panic!("expected run command"),
        }
    }

    #[test]
    fn parse_run_deps_flag_without_value_defaults_to_missing() {
        let cli = Cli::try_parse_from(["wbuild", "run", "zlib", "--deps"]).unwrap();
        match cli.command {
            Commands::Run { deps, .. } => assert_eq!(deps, Some(DepsMode::Missing)),
            _ => panic!("expected run command"),
        }
    }

    #[test]
    fn parse_run_deps_enum_value() {
        let cli = Cli::try_parse_from(["wbuild", "run", "zlib", "--deps=sync"]).unwrap();
        match cli.command {
            Commands::Run { deps, .. } => assert_eq!(deps, Some(DepsMode::Sync)),
            _ => panic!("expected run command"),
        }
    }

    #[test]
    fn parse_run_dependents_flag_without_value_defaults_to_link() {
        let cli = Cli::try_parse_from(["wbuild", "run", "glibc", "--dependents"]).unwrap();
        match cli.command {
            Commands::Run { dependents, .. } => {
                assert_eq!(dependents, Some(DependentsMode::Link))
            }
            _ => panic!("expected run command"),
        }
    }

    #[test]
    fn parse_run_dependents_enum_value() {
        let cli =
            Cli::try_parse_from(["wbuild", "run", "glibc", "--dependents=all"]).unwrap();
        match cli.command {
            Commands::Run { dependents, .. } => {
                assert_eq!(dependents, Some(DependentsMode::All))
            }
            _ => panic!("expected run command"),
        }
    }
}
