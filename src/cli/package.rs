use clap::Parser;

pub const PACKAGE_AFTER_HELP: &str = "\
Examples:
  wright package zlib
  wright package zlib --force
  wright package freetype --print-parts
  echo -e 'curl\nwget' | wright package";

#[derive(Parser, Debug, Clone)]
pub struct PackageArgs {
    /// Paths to plan directories, part names, or @assemblies
    pub targets: Vec<String>,

    /// Force repackaging: overwrite existing archives
    #[arg(long, short)]
    pub force: bool,

    /// Print produced archive paths to stdout after packaging.
    /// Human-readable logs continue to go to stderr so this remains pipe-safe.
    #[arg(long)]
    pub print_parts: bool,
}
