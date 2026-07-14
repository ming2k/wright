use indicatif::MultiProgress;
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;

/// Global multi-progress coordinator. Every progress bar should be registered
/// through this instance so `indicatif` can manage terminal lines without
/// flickering.
pub static MULTI: LazyLock<MultiProgress> = LazyLock::new(MultiProgress::new);

/// Print a terminal line through the progress coordinator so it serializes
/// against active progress bars.  When the draw target is hidden (stderr is
/// not a terminal — pipes, CI, cron), `MultiProgress::println` silently
/// discards lines, so fall back to plain stderr to keep errors and action
/// lines visible.
pub fn term_println(line: &str) {
    if MULTI.is_hidden() {
        eprintln!("{line}");
    } else {
        let _ = MULTI.println(line);
    }
}

/// When set, suppress CLI INFO-level output so that fail-fast failures
/// are not buried under output from tasks already in flight.
pub static SUPPRESS_INFO_OUTPUT: AtomicBool = AtomicBool::new(false);

pub fn source_label(uri: &str) -> String {
    let uri = uri.strip_prefix("git+").unwrap_or(uri);
    let uri = uri.strip_prefix("file://").unwrap_or(uri);
    let uri = uri.split('#').next().unwrap_or(uri);
    let tail = uri
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(uri);
    tail.trim_end_matches(".git").to_string()
}

/// Record byte-progress on an in-flight `cli_span!`. The SpinnerLayer
/// swaps the row from spinner to byte-progress bar the first time
/// `bytes_total` is recorded, then animates as `bytes_done` changes.
pub fn record_bytes(span: &tracing::Span, transferred: u64, total: u64) {
    span.record("bytes_done", transferred);
    if total > 0 {
        span.record("bytes_total", total);
    }
}

/// Emit a Cargo-style action line (e.g. `   Compiling linux-lts`) plus the
/// equivalent structured tracing event for the file log.
///
/// Usage: `cli_action!("Compiling", "{}", plan_name);`
#[macro_export]
macro_rules! cli_action {
    ($verb:expr, $($arg:tt)*) => {{
        if !$crate::util::progress::SUPPRESS_INFO_OUTPUT.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::info!(verb = %$verb, $($arg)*);
        }
    }};
}

/// Open a tracing span that drives a persistent live-status row via the
/// `SpinnerLayer`. The row appears the moment the span enters scope and
/// disappears the moment the span drops. The `verb` is rendered in the
/// 12-col right-aligned bold-green column; the formatted target string
/// follows.
///
/// Long-running work (compile, configure, source download, extract)
/// should use this; one-shot announcements should use [`cli_action!`].
///
/// Usage:
/// ```ignore
/// let _stage = cli_span!("Compiling", "{}", plan_name);
/// // ... do compile work; spinner is live this whole scope
/// // span drops here, spinner clears
/// ```
///
/// For byte-progress (HTTP downloads), record the size fields on the
/// span to swap to a download-bar display:
/// ```ignore
/// let s = cli_span!("Fetching", "{}", filename);
/// s.record("bytes_total", &total);
/// s.record("bytes_done", &done);
/// ```
#[macro_export]
macro_rules! cli_span {
    ($verb:expr, $($arg:tt)*) => {{
        let target = format!($($arg)*);
        // Scroll the action line into history at span-open time (Rule A:
        // present-participle announces start). Rule B (implicit silence
        // on completion) is honored because spans don't emit anything on
        // close — the spinner just disappears.
        if !$crate::util::progress::SUPPRESS_INFO_OUTPUT
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            tracing::info!(verb = %$verb, "{}", target);
        }
        tracing::info_span!(
            "cli_op",
            verb = %$verb,
            target = %target,
            // Pre-declare the byte-progress fields so `Span::record` can
            // update them later without a "field not declared" warning.
            bytes_done = tracing::field::Empty,
            bytes_total = tracing::field::Empty,
        )
    }};
}

/// Print a `warning:` line. Goes through tracing so file logs match.
#[macro_export]
macro_rules! cli_warn {
    ($($arg:tt)*) => {{
        tracing::warn!($($arg)*);
    }};
}

/// Print an `error:` line. Goes through tracing so file logs match.
#[macro_export]
macro_rules! cli_error {
    ($($arg:tt)*) => {{
        tracing::error!($($arg)*);
    }};
}

/// Print a Cargo-style red `Failed` action line at ERROR level. Use for
/// terminal failures (whole workflow gave up). For mid-flight problems
/// that are still recoverable, use [`cli_warn!`]; for outright errors
/// that aren't a workflow conclusion, use [`cli_error!`].
#[macro_export]
macro_rules! cli_failed {
    ($($arg:tt)*) => {{
        tracing::error!(verb = "Failed", $($arg)*);
    }};
}

/// Print a Cargo-style red `Aborted` action line at ERROR level. Use when
/// the user interrupts the workflow (Ctrl+C, SIGTERM).
#[macro_export]
macro_rules! cli_aborted {
    ($($arg:tt)*) => {{
        tracing::error!(verb = "Aborted", $($arg)*);
    }};
}

/// Print a raw CLI message without any level prefix.
#[macro_export]
macro_rules! cli_output {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        $crate::util::progress::term_println(&msg);
    }};
}
