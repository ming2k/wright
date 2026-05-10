use anyhow::Context;
use clap::Parser;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;
use wright::cli::Cli;
use wright::config::GlobalConfig;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // 1. Setup Logging
    let filter = if cli.verbose > 1 {
        EnvFilter::new("trace")
    } else if cli.verbose > 0 {
        EnvFilter::new("debug")
    } else if cli.quiet {
        EnvFilter::new("warn")
    } else {
        EnvFilter::new("info")
    };

    if cli.verbose > 0 {
        tracing_subscriber::fmt()
            .with_writer(wright::util::progress::MultiProgressWriter)
            .with_env_filter(filter)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(wright::util::progress::MultiProgressWriter)
            .without_time()
            .with_target(false)
            .with_level(true)
            .with_env_filter(filter)
            .init();
    }

    // 2. Load Configuration and Dispatch
    let result = async {
        let config = GlobalConfig::load(cli.config.as_deref()).context("failed to load config")?;

        let root_dir = cli.root.clone().unwrap_or_else(|| PathBuf::from("/"));

        // When --root is set and --db is not, redirect the database under the target
        // root. Operating on an alternate root with the host's database silently
        // misrecords every install, so we make this implicit redirection explicit.
        let db_path = cli.db.clone().unwrap_or_else(|| {
            if root_dir == Path::new("/") {
                config.general.db_path.clone()
            } else {
                root_dir.join("var/lib/wright/wright.db")
            }
        });

        wright::commands::dispatch(cli, &config, db_path, root_dir).await
    }
    .await;

    if let Err(e) = result {
        tracing::error!("{}", e);
        std::process::exit(1);
    }
}
