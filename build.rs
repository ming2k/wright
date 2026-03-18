use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::CommandFactory;

#[path = "src/cli/wbuild.rs"]
mod wbuild_cli;
#[path = "src/cli/wrepo.rs"]
mod wrepo_cli;
#[path = "src/cli/wright.rs"]
mod wright_cli;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli/wbuild.rs");
    println!("cargo:rerun-if-changed=src/cli/wrepo.rs");
    println!("cargo:rerun-if-changed=src/cli/wright.rs");

    let out_dir = man_output_dir();
    fs::create_dir_all(&out_dir).expect("failed to create man output directory");

    render::<wright_cli::Cli>("wright", &out_dir);
    render::<wbuild_cli::Cli>("wbuild", &out_dir);
    render::<wrepo_cli::Cli>("wrepo", &out_dir);
}

fn render<T>(name: &str, out_dir: &Path)
where
    T: CommandFactory,
{
    let cmd = decorate_command(T::command(), name.to_owned(), name.to_owned());
    clap_mangen::generate_to(cmd, out_dir)
        .unwrap_or_else(|err| panic!("failed to write man pages for {name}: {err}"));
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

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    manifest_dir.join("target").join("man")
}
