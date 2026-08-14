use clap::Parser;
use tracing_subscriber::EnvFilter;
use wright::cli::Cli;
use wright::config::GlobalConfig;
use wright::error::WrightError;
use wright::util::logging::{
    format_batch_failure_report, format_error, format_failure_report, today_log_path,
};
use wright::util::progress::MULTI;

fn main() {
    // Isolation setup must begin before Tokio creates worker threads. The
    // helper then owns every fork/unshare/mount operation in a fresh,
    // single-threaded process.
    if wright::isolation::is_helper_process() {
        wright::isolation::run_helper_process();
    }
    run_cli();
}

#[tokio::main]
async fn run_cli() {
    let cli = Cli::parse();

    // 1. Load Configuration First — pre-logging, so emit the error line
    //    directly through format_error rather than the tracing layer.
    let config = match GlobalConfig::load(cli.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            wright::errln!("{}", format_error(&format!("failed to load config: {}", e)));
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
        // printing the multi-line block ourselves below. The error text is
        // the fully-flattened chain so foreign errors whose Display omits
        // their sources (gix) still log the root cause.
        tracing::error!(
            event = "command.failed",
            error = %wright::util::logging::flatten_error_causes(&e),
            trace_id = %trace_id,
            "command failed"
        );

        // Multi-line Cargo-style report on the terminal. A settled batch
        // with several failed tasks gets its own aggregated report that
        // re-lists every per-task failure.
        let log_path = today_log_path(&logs_dir);
        let mut lines = match &e {
            WrightError::BatchFailures(failures) => {
                format_batch_failure_report(failures, &log_path)
            }
            _ => format_failure_report(&e, &log_path),
        };
        // A failed workflow defers its step-timing report so it closes the
        // failure block here instead of splitting the error output mid-run
        // (see util::timing::WorkflowTiming::log_report).
        if let Some(timing_lines) = wright::util::timing::emit_deferred_failure() {
            lines.push(String::new());
            lines.extend(timing_lines);
        }
        for line in lines {
            wright::util::progress::term_println(&line);
        }

        std::process::exit(1);
    }
}
