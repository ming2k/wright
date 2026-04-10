use std::env;
use std::fs;
use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand};

// Mock the structures needed by the CLI definition
mod wbuild {
    use clap::{Parser, ValueEnum};

    #[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
    pub enum DepsMode { None, Missing, Sync, All }
    #[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
    pub enum DependentsModeArg { Link, All }

    #[derive(Parser, Debug, Clone)]
    pub struct RunArgs {
        pub targets: Vec<String>,
        #[arg(long)] pub stage: Vec<String>,
        #[arg(long, conflicts_with = "stage")] pub skip_check: bool,
        #[arg(long, short = 'c')] pub clean: bool,
        #[arg(long, short)] pub force: bool,
        #[arg(long, short, num_args = 0..=1, default_missing_value = "")] pub resume: Option<String>,
        #[arg(short = 'w', long, default_value = "0")] pub dockyards: usize,
        #[arg(long)] pub mvp: bool,
        #[arg(long)] pub print_archives: bool,
        #[arg(long)] pub clear_sessions: bool,
    }

    #[derive(Parser, Debug, Clone)]
    pub struct ResolveArgs {
        pub targets: Vec<String>,
        #[arg(short = 's', long = "self")] pub include_self: bool,
        #[arg(short = 'd', long = "deps", value_enum, num_args = 0..=1, default_missing_value = "missing")] pub deps: Option<DepsMode>,
        #[arg(long = "dependents", value_enum, num_args = 0..=1, default_missing_value = "link")] pub dependents: Option<DependentsModeArg>,
        #[arg(long)] pub depth: Option<usize>,
        #[arg(long, short = 't', conflicts_with_all = ["deps", "dependents", "include_self"])] pub tree: bool,
    }

    #[derive(Parser, Debug, Clone)]
    pub struct CheckArgs { pub targets: Vec<String> }
    #[derive(Parser, Debug, Clone)]
    pub struct FetchArgs { pub targets: Vec<String> }
    #[derive(Parser, Debug, Clone)]
    pub struct ChecksumArgs { pub targets: Vec<String> }
    #[derive(Parser, Debug, Clone)]
    pub struct PruneArgs {
        #[arg(long)] pub untracked: bool,
        #[arg(long)] pub latest: bool,
        #[arg(long)] pub apply: bool,
    }
}

mod wright {
    use clap::{Subcommand, ValueEnum};

    #[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
    pub enum PrefixModeArg { Indent, Depth, None }

    #[derive(Subcommand, Debug, Clone)]
    pub enum Commands {
        Install { parts: Vec<String>, #[arg(long)] force: bool, #[arg(long)] nodeps: bool },
        Apply { targets: Vec<String>, #[arg(long)] force_build: bool, #[arg(long)] force_install: bool, #[arg(long)] nodeps: bool },
        Upgrade { #[arg(required = true)] parts: Vec<String>, #[arg(long)] force: bool, #[arg(long)] version: Option<String> },
        Remove { #[arg(required = true)] parts: Vec<String>, #[arg(long)] force: bool, #[arg(long, short)] recursive: bool, #[arg(long, short = 'c')] cascade: bool },
        Deps { part: Option<String>, #[arg(long, short)] reverse: bool, #[arg(long, short, default_value = "0")] depth: usize, #[arg(long, short)] filter: Option<String>, #[arg(long, short)] all: bool, #[arg(long, value_enum, default_value_t = PrefixModeArg::Indent)] prefix: PrefixModeArg, #[arg(long)] prune: Vec<String> },
        List { #[arg(long, short)] long: bool, #[arg(long, short)] roots: bool, #[arg(long, short)] assumed: bool, #[arg(long, short)] orphans: bool },
        Query { part: String },
        Search { keyword: String },
        Files { part: String },
        Owner { file: String },
        Verify { part: Option<String> },
        Doctor,
        Assume { name: String, version: String },
        Unassume { name: String },
        Mark { #[arg(required = true)] parts: Vec<String>, #[arg(long)] as_dependency: bool, #[arg(long)] as_manual: bool },
        History { part: Option<String> },
        Sysupgrade { #[arg(long, short = 'n')] dry_run: bool },
    }
}

#[derive(Parser)]
#[command(name = "wright", version)]
struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[arg(long, global = true)] pub root: Option<PathBuf>,
    #[arg(long, global = true)] pub config: Option<PathBuf>,
    #[arg(long, global = true)] pub db: Option<PathBuf>,
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)] pub verbose: u8,
    #[arg(long, global = true)] pub quiet: bool,
}

#[derive(Subcommand)]
enum Commands {
    #[command(flatten)] System(wright::Commands),
    Build(wbuild::RunArgs),
    #[command(subcommand)] Plan(PlanCommands),
    #[command(subcommand)] Inventory(InventoryCommands),
}

#[derive(Subcommand)]
enum PlanCommands {
    Resolve(wbuild::ResolveArgs),
    Check(wbuild::CheckArgs),
    Fetch(wbuild::FetchArgs),
    Checksum(wbuild::ChecksumArgs),
}

#[derive(Subcommand)]
enum InventoryCommands {
    Prune(wbuild::PruneArgs),
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = man_output_dir();
    fs::create_dir_all(&out_dir).expect("failed to create man output directory");
    let cmd = Cli::command();
    let cmd = decorate_command(cmd, "wright".to_string(), "wright".to_string());
    clap_mangen::generate_to(cmd, out_dir).expect("failed to generate man pages");
}

fn decorate_command(cmd: clap::Command, page_name: String, command_name: String) -> clap::Command {
    cmd.display_name(page_name.clone())
        .bin_name(command_name.clone())
        .mut_subcommands(|subcommand| {
            let child_page_name = format!("{}-{}", page_name, subcommand.get_name());
            let child_command_name = format!("{} {}", command_name, subcommand.get_name());
            decorate_command(subcommand, child_page_name, child_command_name)
        })
}

fn man_output_dir() -> PathBuf {
    if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir).join("man");
    }
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    manifest_dir.join("target").join("man")
}
