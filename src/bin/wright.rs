use clap::Parser;
use tracing_subscriber::EnvFilter;
use wright::cli::Cli;
use wright::config::GlobalConfig;
use wright::util::logging::{format_error, format_failure_report, today_log_path};
use wright::util::progress::MULTI;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // 1. Load Configuration First — pre-logging, so emit the error line
    //    directly through format_error rather than the tracing layer.
    let config = match GlobalConfig::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", format_error(&format!("failed to load config: {}", e)));
            std::process::exit(1);
        }
    };

    // 2. Setup Logging (File + Console Handlers)
    let filter = if cli.verbose > 1 {
        EnvFilter::new("trace")
    } else if cli.verbose > 0 {
        EnvFilter::new("debug")
    } else if cli.quiet {
        EnvFilter::new("warn")
    } else {
        EnvFilter::new("info")
    };

    let logs_dir = config.general.logs_dir.clone();
    let _log_guard = wright::util::logging::init_logging(&logs_dir, filter);

    // Generate a trace ID for this command invocation and propagate it.
    let trace_id = wright::util::logging::init_trace_id();
    let span = tracing::info_span!("wright_command", trace_id = %trace_id);
    let _guard = span.enter();

    // 3. Dispatch Command
    let result = wright::cli::dispatch(cli, &config).await;

    if let Err(e) = result {
        // Wipe any active progress bars so they don't bleed into the
        // failure block, then suppress CLI INFO so in-flight tasks can't
        // race with our final output.
        let _ = MULTI.clear();
        wright::util::logging::suppress_cli_output();

        // Structured event for the file log; ERROR-level but no `verb`,
        // and we'll suppress the CLI layer's `error: …` render by
        // printing the multi-line block ourselves below.
        tracing::error!(
            event = "command.failed",
            error = %e,
            trace_id = %trace_id,
            "command failed"
        );

        // Multi-line Cargo-style report on the terminal.
        let log_path = today_log_path(&logs_dir);
        for line in format_failure_report(&e, &log_path) {
            wright::util::progress::term_println(&line);
        }

        std::process::exit(1);
    }
}
