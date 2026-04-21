use std::path::PathBuf;
use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;
use wright::cli::Cli;
use wright::config::GlobalConfig;

#[tokio::main]
async fn main() -> Result<()> {
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

    // 2. Load Configuration
    let config = GlobalConfig::load(cli.config.as_deref()).context("failed to load config")?;

    let installed_db_path = cli
        .db
        .clone()
        .unwrap_or_else(|| config.general.installed_db_path.clone());
    let root_dir = cli.root.clone().unwrap_or_else(|| PathBuf::from("/"));

    // 3. Dispatch to Command Handlers
    wright::commands::dispatch(cli, &config, installed_db_path, root_dir).await
}
