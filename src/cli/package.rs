use clap::Parser;
use std::path::PathBuf;

pub const PACKAGE_AFTER_HELP: &str = "\
Examples:
  wright package zlib
  wright package zlib --force
  wright package zlib --out-dir /tmp/wright-parts
  wright package freetype --print-parts
  echo -e 'curl\nwget' | wright package";

#[derive(Parser, Debug, Clone)]
pub struct PackageArgs {
    /// Paths to plan directories or part names
    pub targets: Vec<String>,

    /// Force repackaging: re-slice outputs from staging and overwrite existing archives
    #[arg(long, short)]
    pub force: bool,

    /// Write produced archives to this directory instead of general.parts_dir
    #[arg(long, value_name = "PATH")]
    pub out_dir: Option<PathBuf>,

    /// Print produced archive paths to stdout after packaging.
    /// Human-readable logs continue to go to stderr so this remains pipe-safe.
    #[arg(long)]
    pub print_parts: bool,
}
