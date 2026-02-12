use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::{info, warn, debug};
use walkdir::WalkDir;

use wright::package::manifest::PackageManifest;
use wright::util::checksum;

#[derive(Parser)]
#[command(name = "wright-repo", about = "wright repository management tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate repository index from a hold tree
    Generate {
        /// Path to the hold tree (root containing core/base/extra)
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Output directory for the index (default: path/packages)
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Generate { path, output } => {
            generate_repo(&path, output)?;
        }
    }

    Ok(())
}

fn generate_repo(hold_root: &Path, output_dir: Option<PathBuf>) -> Result<()> {
    let hold_root = hold_root.canonicalize().context("failed to resolve hold path")?;
    info!("Generating repo for hold tree: {}", hold_root.display());

    let pkg_dir = output_dir.unwrap_or_else(|| hold_root.join("packages"));
    if !pkg_dir.exists() {
        info!("Creating packages directory: {}", pkg_dir.display());
        std::fs::create_dir_all(&pkg_dir).context("failed to create packages directory")?;
    }

    let mut pkg_count = 0;
    
    // Scan for package.toml files
    for entry in WalkDir::new(&hold_root)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_str().map(|s| s == "packages").unwrap_or(false))
    {
        let entry = entry?;
        if entry.file_name() == "package.toml" {
            let manifest = match PackageManifest::from_file(entry.path()) {
                Ok(m) => m,
                Err(e) => {
                    warn!("Skipping invalid manifest {}: {}", entry.path().display(), e);
                    continue;
                }
            };

            // Check if binary exists
            let archive_name = manifest.archive_filename();
            let archive_path = pkg_dir.join(&archive_name);
            
            if archive_path.exists() {
                let hash = checksum::sha256_file(&archive_path)?;
                info!("Found built package: {} (SHA256: {})", archive_name, hash);
            } else {
                debug!("Package source found but no binary: {}", manifest.package.name);
            }
            
            pkg_count += 1;
        }
    }

    info!("Scan complete. Found {} package definitions in hold tree.", pkg_count);
    info!("Binary packages should be placed in: {}", pkg_dir.display());
    
    Ok(())
}
