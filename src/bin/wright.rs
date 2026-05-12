use clap::Parser;
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
        let config = GlobalConfig::load(cli.config.as_deref()).map_err(|e| {
            wright::error::WrightError::ConfigError(format!("failed to load config: {}", e))
        })?;

        wright::commands::dispatch(cli, &config).await
    }
    .await;

    if let Err(e) = result {
        eprintln!("ERROR: {}", e);
        std::process::exit(1);
    }
}
