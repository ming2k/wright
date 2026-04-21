use std::env;
use std::fs;
use std::path::PathBuf;

use clap::CommandFactory;

#[path = "src/cli/mod.rs"]
mod cli;

use cli::Cli;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cli/mod.rs");
    println!("cargo:rerun-if-changed=src/cli/build.rs");
    println!("cargo:rerun-if-changed=src/cli/wright.rs");

    // 1. Generate man pages
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

    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    manifest_dir.join("target").join("man")
}
