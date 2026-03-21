use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

const WBUILD_AFTER_HELP: &str = "\
Workflows:
  Build a part:            wbuild run zlib
  Build and install:       wbuild run zlib -i
  Resolve + build:         wbuild resolve openssl --self --dependents | wbuild run -i
  Full reverse cascade:    wbuild resolve glibc --self --dependents=all --depth=0 | wbuild run --force -i
  Resume after failure:    wbuild resolve glibc --self --dependents=all --depth=0 | wbuild run --resume -i
  Dependency tree:         wbuild resolve zlib --tree
  Validate plans:          wbuild check ./plans/zlib

Targets may be plan names, plan directories, or `@assembly` references.";
const WBUILD_RUN_AFTER_HELP: &str = "\
Examples:
  wbuild run zlib
  wbuild run zlib -i
  wbuild run zlib --force --clean
  wbuild run freetype --mvp --stage=configure
  wbuild resolve openssl --self --dependents | wbuild run -i
  echo -e 'curl\\nwget' | wbuild run --force -i

Resume after partial failure (auto-detect session):
  wbuild resolve pcre2 --self --dependents --depth=0 | wbuild run --resume -i
Resume with explicit session hash:
  wbuild resolve pcre2 --self --dependents --depth=0 | wbuild run --resume abc123... -i";
const WBUILD_RESOLVE_AFTER_HELP: &str = "\
Examples:
  wbuild resolve zlib --self --deps
  wbuild resolve zlib --self --deps=sync
  wbuild resolve openssl --self --dependents
  wbuild resolve glibc --self --dependents=all --depth=0
  wbuild resolve zlib --self --deps=all

Pipe into wbuild run:
  wbuild resolve openssl --self --dependents | wbuild run -i

Dependency tree (static plan.toml analysis):
  wbuild resolve zlib --tree
  wbuild resolve gtk4 --tree --depth=2";
const WBUILD_CHECK_AFTER_HELP: &str = "\
Examples:
  wbuild check zlib
  wbuild check ./plans/zlib";
const WBUILD_FETCH_AFTER_HELP: &str = "\
Examples:
  wbuild fetch zlib
  wbuild fetch ./plans/zlib";
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
        long_about = "Build parts from plans.\n\nTargets may be plan names, plan directories, or `@assembly` references. Reads additional targets from stdin when piped (one per line). Use `wbuild resolve` to expand dependencies/dependents before piping into `wbuild run`.",
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

        /// Resume a previous build session: skip parts that were already
        /// successfully built and installed. Optionally pass a session hash
        /// (printed on failure); without a hash, auto-detects from the
        /// current build set.
        #[arg(long, short, num_args = 0..=1, default_missing_value = "")]
        resume: Option<String>,

        /// Max number of concurrent dockyards. Only parts with no
        /// direct or indirect dependency relationship run simultaneously;
        /// the scheduler enforces ordering automatically.
        /// 0 = auto-detect CPU count.
        #[arg(short = 'w', long, default_value = "0")]
        dockyards: usize,

        /// Automatically install each part after a successful build
        #[arg(short = 'i', long)]
        install: bool,

        /// Build using the MVP dependency set from inline [mvp.dependencies]
        /// or sibling mvp.toml without requiring a dependency cycle to trigger it
        #[arg(long)]
        mvp: bool,
    },
    /// Resolve targets and expand dependencies/dependents.
    /// Outputs plan names to stdout, one per line.
    /// Pipe into `wbuild run` to build the resolved set.
    #[command(
        long_about = "Resolve targets and expand their dependency graph.\n\nOutputs plan names to stdout (one per line) for piping into `wbuild run`. Expansion flags control which parts are included: `--deps` adds upstream dependencies, `--dependents` adds downstream dependents. Use `--self` to include the listed targets themselves.\n\nWith `--tree`, switches to a visual dependency tree from hold-tree `plan.toml` files (static analysis — does not read the installed part database).",
        after_help = WBUILD_RESOLVE_AFTER_HELP
    )]
    Resolve {
        /// Paths to plan directories, part names, or @assemblies
        targets: Vec<String>,

        /// Include the listed parts themselves in the output
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

        /// Expand downstream dependents (installed parts only).
        /// `link` follows ABI-sensitive link dependents.
        /// `all` also follows runtime and build dependents.
        #[arg(
            long = "dependents",
            value_enum,
            num_args = 0..=1,
            default_missing_value = "link"
        )]
        dependents: Option<DependentsMode>,

        /// Maximum expansion depth. `0` means unlimited.
        /// If omitted, reverse-dependent expansion defaults to depth 1;
        /// other expansions default to unlimited.
        #[arg(long)]
        depth: Option<usize>,

        /// Show a visual dependency tree from hold-tree plan.toml files.
        /// This is a static analysis mode — it does not read the installed
        /// part database.
        #[arg(long, short = 't', conflicts_with_all = ["deps", "dependents", "include_self"])]
        tree: bool,
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
    fn parse_run_accepts_targets() {
        let cli = Cli::try_parse_from(["wbuild", "run", "zlib"]).unwrap();
        match cli.command {
            Commands::Run { targets, .. } => assert_eq!(targets, vec!["zlib"]),
            _ => panic!("expected run command"),
        }
    }

    #[test]
    fn parse_resolve_deps_flag_without_value_defaults_to_missing() {
        let cli = Cli::try_parse_from(["wbuild", "resolve", "zlib", "--self", "--deps"]).unwrap();
        match cli.command {
            Commands::Resolve { deps, .. } => assert_eq!(deps, Some(DepsMode::Missing)),
            _ => panic!("expected resolve command"),
        }
    }

    #[test]
    fn parse_resolve_deps_enum_value() {
        let cli =
            Cli::try_parse_from(["wbuild", "resolve", "zlib", "--self", "--deps=sync"]).unwrap();
        match cli.command {
            Commands::Resolve { deps, .. } => assert_eq!(deps, Some(DepsMode::Sync)),
            _ => panic!("expected resolve command"),
        }
    }

    #[test]
    fn parse_resolve_dependents_flag_without_value_defaults_to_link() {
        let cli =
            Cli::try_parse_from(["wbuild", "resolve", "glibc", "--self", "--dependents"]).unwrap();
        match cli.command {
            Commands::Resolve { dependents, .. } => {
                assert_eq!(dependents, Some(DependentsMode::Link))
            }
            _ => panic!("expected resolve command"),
        }
    }

    #[test]
    fn parse_resolve_dependents_enum_value() {
        let cli = Cli::try_parse_from(["wbuild", "resolve", "glibc", "--self", "--dependents=all"])
            .unwrap();
        match cli.command {
            Commands::Resolve { dependents, .. } => {
                assert_eq!(dependents, Some(DependentsMode::All))
            }
            _ => panic!("expected resolve command"),
        }
    }

    #[test]
    fn parse_resolve_tree_flag() {
        let cli = Cli::try_parse_from(["wbuild", "resolve", "zlib", "--tree"]).unwrap();
        match cli.command {
            Commands::Resolve { tree, .. } => assert!(tree),
            _ => panic!("expected resolve command"),
        }
    }

    #[test]
    fn parse_resolve_tree_with_depth() {
        let cli =
            Cli::try_parse_from(["wbuild", "resolve", "zlib", "--tree", "--depth=2"]).unwrap();
        match cli.command {
            Commands::Resolve { tree, depth, .. } => {
                assert!(tree);
                assert_eq!(depth, Some(2));
            }
            _ => panic!("expected resolve command"),
        }
    }

    #[test]
    fn parse_resolve_tree_conflicts_with_deps() {
        let result =
            Cli::try_parse_from(["wbuild", "resolve", "zlib", "--tree", "--deps"]);
        assert!(result.is_err());
    }

    #[test]
    fn parse_resolve_tree_conflicts_with_self() {
        let result =
            Cli::try_parse_from(["wbuild", "resolve", "zlib", "--tree", "--self"]);
        assert!(result.is_err());
    }
}
