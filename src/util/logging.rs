use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Style};
use std::collections::HashMap;
use std::fmt;
use std::io::IsTerminal;
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use uuid::Uuid;

// ─── Color Support ─────────────────────────────────────────────────────────

/// Colors are enabled iff stderr is a TTY and `NO_COLOR` is unset
/// (see <https://no-color.org>).
pub static USE_COLOR: LazyLock<bool> = LazyLock::new(|| {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
});

/// Width of the right-aligned verb column. Matches Cargo (12).
pub const VERB_WIDTH: usize = 12;

/// Render a Cargo-style action line: `{verb:>12} {msg}`.
/// Standard verbs are bold green; the terminal failure verbs
/// `Failed` and `Aborted` are bold red.
pub fn format_action(verb: &str, msg: &str) -> String {
    let padded = format!("{:>w$}", verb, w = VERB_WIDTH);
    if *USE_COLOR {
        let style = if matches!(verb, "Failed" | "Aborted") {
            Style::new().red().bold()
        } else {
            Style::new().green().bold()
        };
        format!("{} {}", padded.style(style), msg)
    } else {
        format!("{} {}", padded, msg)
    }
}

/// `warning: <msg>` with the prefix in bold yellow.
pub fn format_warn(msg: &str) -> String {
    let prefix = "warning:";
    if *USE_COLOR {
        format!("{} {}", prefix.style(Style::new().yellow().bold()), msg)
    } else {
        format!("{} {}", prefix, msg)
    }
}

/// `error: <msg>` with the prefix in bold red.
pub fn format_error(msg: &str) -> String {
    let prefix = "error:";
    if *USE_COLOR {
        format!("{} {}", prefix.style(Style::new().red().bold()), msg)
    } else {
        format!("{} {}", prefix, msg)
    }
}

/// Build a multi-line failure report for a terminal error, in the style
/// of `cargo` / `anyhow`.
///
/// The single-string error chains produced by `WrightError` (which nest via
/// `format!("{}: {}", prefix, inner)`) are split on `": "`, type-prefix
/// segments (`forge error`, `deploy error`, …) are dropped, and the
/// remaining causes render under a numbered `Caused by:` block. The
/// trailing line points at the structured log file for the full trace.
///
/// Layout follows `cargo`: zero-indent `error:` headline, blank
/// separator, `Caused by:` block with 4-space numbered entries, blank
/// separator, log-file hint. No verb-column alignment — failures
/// prioritize information density over visual scanning.
///
/// Returns a `Vec<String>` so the caller can print each line through
/// `MULTI.println` — which serializes against active progress bars.
pub fn format_failure_report(err: &dyn std::fmt::Display, log_path: &std::path::Path) -> Vec<String> {
    let chain = split_error_chain(&format!("{}", err));
    let (head, causes) = match chain.split_first() {
        Some((h, rest)) => (h.clone(), rest.to_vec()),
        None => ("command failed".to_string(), Vec::new()),
    };

    let mut lines = Vec::new();
    lines.push(format_error(&head));
    if !causes.is_empty() {
        lines.push(String::new());
        lines.push("Caused by:".to_string());
        if causes.len() == 1 {
            lines.push(format!("    {}", causes[0]));
        } else {
            for (i, c) in causes.iter().enumerate() {
                lines.push(format!("    {}: {}", i, c));
            }
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "See {} for the full trace.",
        log_path.display()
    ));
    lines
}

/// Split a colon-joined error chain into segments, stripping the
/// `WrightError` variant prefixes (`forge error`, `deploy error`, etc.) so
/// the user sees only the actual cause messages.
fn split_error_chain(s: &str) -> Vec<String> {
    s.split(": ")
        .map(str::trim)
        .filter(|seg| !seg.is_empty() && !is_wright_error_prefix(seg))
        .map(str::to_string)
        .collect()
}

fn is_wright_error_prefix(seg: &str) -> bool {
    matches!(
        seg,
        "parse error"
            | "I/O error"
            | "database error"
            | "forge error"
            | "deploy error"
            | "remove error"
            | "part error"
            | "config error"
            | "lock error"
            | "version error"
            | "dependency error"
            | "upgrade error"
            | "script error"
            | "validation error"
            | "isolation error"
            | "network error"
            | "TOML deserialization error"
            | "SQLite error"
    )
}

/// Today's rolling-daily file path for the wright log.
/// `tracing_appender::rolling::daily(dir, "wright.log")` writes to
/// `dir/wright.log.YYYY-MM-DD`.
pub fn today_log_path(logs_dir: &std::path::Path) -> std::path::PathBuf {
    let today = chrono::Local::now().format("%Y-%m-%d");
    logs_dir.join(format!("wright.log.{}", today))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_chain_drops_wrighterror_prefixes() {
        let s = "forge error: task 'bison' failed: forge error: forge bison: \
                 forge error: failed to clean forge directory \
                 /var/tmp/wright/workshop/bison-3.8.2: \
                 Device or resource busy (os error 16)";
        assert_eq!(
            split_error_chain(s),
            vec![
                "task 'bison' failed",
                "forge bison",
                "failed to clean forge directory /var/tmp/wright/workshop/bison-3.8.2",
                "Device or resource busy (os error 16)",
            ]
        );
    }

    #[test]
    fn split_chain_handles_single_message() {
        assert_eq!(
            split_error_chain("plain message"),
            vec!["plain message"]
        );
    }

    #[test]
    fn failure_report_single_cause_unnumbered() {
        let err = "forge error: task 'bison' failed: \
                   Device or resource busy (os error 16)";
        let path = std::path::PathBuf::from("/var/log/wright/wright.log.2026-05-15");
        let lines = format_failure_report(&err, &path);
        // headline / blank / Caused by / cause / blank / see ...
        assert_eq!(lines.len(), 6);
        assert!(lines[0].contains("error:"), "headline: {}", lines[0]);
        assert!(lines[0].contains("task 'bison' failed"), "headline: {}", lines[0]);
        assert!(lines[1].is_empty());
        assert_eq!(lines[2], "Caused by:");
        assert_eq!(lines[3], "    Device or resource busy (os error 16)");
        assert!(lines[4].is_empty());
        assert!(lines[5].starts_with("See "));
        assert!(lines[5].contains("/var/log/wright/wright.log.2026-05-15"));
    }

    #[test]
    fn failure_report_multi_cause_numbered() {
        let err = "forge error: task 'bison' failed: \
                   forge error: forge bison: \
                   forge error: failed to clean forge directory /var/tmp/wright/workshop/bison-3.8.2: \
                   Device or resource busy (os error 16)";
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        // headline / blank / Caused by / 0 / 1 / 2 / blank / see ...
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[2], "Caused by:");
        assert_eq!(lines[3], "    0: forge bison");
        assert!(lines[4].starts_with("    1: failed to clean forge directory"));
        assert_eq!(lines[5], "    2: Device or resource busy (os error 16)");
    }

    #[test]
    fn failure_report_with_no_chain_omits_caused_by() {
        let err = "plain failure";
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        // headline / blank / see ...
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("plain failure"));
        assert!(lines[1].is_empty());
        assert!(lines[2].starts_with("See "));
    }
}

// ─── Trace ID Management ───────────────────────────────────────────────────

thread_local! {
    static TRACE_ID: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub fn init_trace_id() -> String {
    let id = Uuid::new_v4().to_string();
    TRACE_ID.with(|t| *t.borrow_mut() = Some(id.clone()));
    id
}

pub fn current_trace_id() -> Option<String> {
    TRACE_ID.with(|t| t.borrow().clone())
}

pub fn set_trace_id(id: String) {
    TRACE_ID.with(|t| *t.borrow_mut() = Some(id));
}

pub fn clear_trace_id() {
    TRACE_ID.with(|t| *t.borrow_mut() = None);
}

// ─── CLI Output Layer ──────────────────────────────────────────────────────

static SUPPRESS_CLI: AtomicBool = AtomicBool::new(false);

/// Silence ALL CLI-layer output. Use this on the terminal-failure path
/// after `MULTI.clear()` so in-flight cleanup-path tracing events can't
/// race with the multi-line failure block we're about to print.
///
/// File-layer logging is unaffected.
pub fn suppress_cli_output() {
    SUPPRESS_CLI.store(true, Ordering::Relaxed);
}

pub fn resume_cli_output() {
    SUPPRESS_CLI.store(false, Ordering::Relaxed);
}

/// Tracing layer that renders events as Cargo-style action lines.
///
/// Convention: an INFO event emits CLI output iff it carries a `verb` field.
/// Events without `verb` go to the file log only — this forces deliberate
/// opt-in for anything user-facing.
///
/// WARN and ERROR events always emit, regardless of the `verb` field.
pub struct CliOutputLayer;

impl CliOutputLayer {
    pub fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for CliOutputLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if SUPPRESS_CLI.load(Ordering::Relaxed) {
            return;
        }
        let level = *event.metadata().level();

        let mut v = FieldExtractor::default();
        event.record(&mut v);

        let msg = v.msg.unwrap_or_default();
        // For ERROR/WARN levels, fold the `error = %e` field into the body
        // so users actually see the cause; otherwise it disappears into
        // the structured log file.
        let body = match (msg.as_str(), v.error.as_deref()) {
            ("", Some(err)) => err.to_string(),
            (m, Some(err)) if !m.is_empty() && m != err => format!("{}: {}", m, err),
            (m, _) => m.to_string(),
        };
        // When `verb` is present it always wins — `format_action` already
        // colors `Failed`/`Aborted` red, so an ERROR-level event with a
        // verb renders as a Cargo-style action line, not as `error: …`.
        let line = if let Some(verb) = v.verb.as_deref() {
            Some(format_action(verb, &body))
        } else {
            match level {
                tracing::Level::ERROR => Some(format_error(&body)),
                tracing::Level::WARN => Some(format_warn(&body)),
                _ => None,
            }
        };

        if let Some(line) = line
            && !line.trim().is_empty()
        {
            let _ = crate::util::progress::MULTI.println(line);
        }
    }
}

#[derive(Default)]
struct FieldExtractor {
    msg: Option<String>,
    verb: Option<String>,
    error: Option<String>,
}

impl Visit for FieldExtractor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let s = format!("{:?}", value);
        match field.name() {
            "message" => self.msg = Some(s),
            "verb" => self.verb = Some(strip_quotes(&s)),
            "error" => self.error = Some(strip_quotes(&s)),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.msg = Some(value.to_string()),
            "verb" => self.verb = Some(value.to_string()),
            "error" => self.error = Some(value.to_string()),
            _ => {}
        }
    }
}

fn strip_quotes(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .map(|t| t.to_string())
        .unwrap_or_else(|| s.to_string())
}

// ─── Spinner Layer ─────────────────────────────────────────────────────────

/// Tracing layer that drives a live `indicatif::MultiProgress` display from
/// the set of currently-open spans. Each span carrying `verb` and `target`
/// fields gets an attached `ProgressBar`; the bar is created on `on_new_span`
/// and finished on `on_close`. Long-running work (per-stage spans, per-source
/// fetch spans) therefore appears as a persistent row that vanishes when
/// the work completes — no manual `ProgressBar` plumbing through call stacks.
///
/// Span field conventions:
///   - `verb`         (required for display): action verb (e.g. `Compiling`)
///   - `target`       (required for display): what the work is on
///   - `bytes_done`   (optional): downloaded/processed bytes
///   - `bytes_total`  (optional): total bytes — recording this swaps the
///                                row from a spinner to a download bar
pub struct SpinnerLayer {
    bars: Mutex<HashMap<span::Id, BarState>>,
}

struct BarState {
    bar: ProgressBar,
    has_byte_total: bool,
}

impl SpinnerLayer {
    pub fn new() -> Self {
        Self {
            bars: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for SpinnerLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for SpinnerLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, _ctx: Context<'_, S>) {
        if SUPPRESS_CLI.load(Ordering::Relaxed) {
            return;
        }
        let mut v = SpanFieldExtractor::default();
        attrs.record(&mut v);
        let Some(verb) = v.verb else { return };
        let target = v.target.unwrap_or_default();

        let bar = crate::util::progress::MULTI.add(ProgressBar::new_spinner());
        bar.set_style(spinner_style());
        // No verb-column padding on live rows — the cargo-style 12-col
        // alignment exists for scrolling action lines where the eye scans
        // a column down a static log, not for animated single-row indicators.
        bar.set_prefix(verb);
        bar.set_message(target);
        bar.enable_steady_tick(std::time::Duration::from_millis(120));

        self.bars.lock().unwrap().insert(
            id.clone(),
            BarState {
                bar,
                has_byte_total: false,
            },
        );
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, _ctx: Context<'_, S>) {
        let mut v = SpanFieldExtractor::default();
        values.record(&mut v);

        let mut guard = self.bars.lock().unwrap();
        let Some(state) = guard.get_mut(id) else {
            return;
        };
        if let Some(done) = v.bytes_done {
            state.bar.set_position(done);
        }
        if let Some(total) = v.bytes_total {
            state.bar.set_length(total);
            if !state.has_byte_total {
                state.bar.set_style(bar_style());
                state.has_byte_total = true;
            }
        }
        if let Some(target) = v.target {
            state.bar.set_message(target);
        }
    }

    fn on_close(&self, id: span::Id, _ctx: Context<'_, S>) {
        if let Some(state) = self.bars.lock().unwrap().remove(&id) {
            state.bar.finish_and_clear();
        }
    }
}

fn spinner_style() -> ProgressStyle {
    if *USE_COLOR {
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {prefix:.green.bold} {msg} {elapsed:.dimmed}")
            .expect("valid spinner template")
    } else {
        ProgressStyle::default_spinner()
            .template("{spinner} {prefix} {msg} {elapsed}")
            .expect("valid spinner template")
    }
}

fn bar_style() -> ProgressStyle {
    let template = if *USE_COLOR {
        "{spinner:.green} {prefix:.green.bold} {msg} {elapsed:.dimmed} [{wide_bar:.cyan/blue}] {bytes}/{total_bytes}"
    } else {
        "{spinner} {prefix} {msg} {elapsed} [{wide_bar}] {bytes}/{total_bytes}"
    };
    ProgressStyle::default_bar()
        .template(template)
        .expect("valid bar template")
        .progress_chars("#\u{003e}-")
}

#[derive(Default)]
struct SpanFieldExtractor {
    verb: Option<String>,
    target: Option<String>,
    bytes_done: Option<u64>,
    bytes_total: Option<u64>,
}

impl Visit for SpanFieldExtractor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let s = format!("{:?}", value);
        match field.name() {
            "verb" => self.verb = Some(strip_quotes(&s)),
            "target" => self.target = Some(strip_quotes(&s)),
            _ => {}
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "verb" => self.verb = Some(value.to_string()),
            "target" => self.target = Some(value.to_string()),
            _ => {}
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "bytes_done" => self.bytes_done = Some(value),
            "bytes_total" => self.bytes_total = Some(value),
            _ => {}
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if value >= 0 {
            self.record_u64(field, value as u64);
        }
    }
}

// ─── Subscriber Initialization ─────────────────────────────────────────────

pub fn init_logging(
    log_dir: &std::path::Path,
    cli_filter: tracing_subscriber::EnvFilter,
) -> tracing_appender::non_blocking::WorkerGuard {
    let _ = std::fs::create_dir_all(log_dir);

    // 1. File handler — structured JSON for diagnostics. DEBUG by default,
    //    overridable via WRIGHT_LOG.
    let file_appender = tracing_appender::rolling::daily(log_dir, "wright.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let file_filter = tracing_subscriber::EnvFilter::try_from_env("WRIGHT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));

    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
        .with_current_span(true)
        .with_span_list(false)
        .with_writer(non_blocking)
        .flatten_event(true)
        .with_filter(file_filter);

    // 2. CLI scrolling output — Cargo-style verb lines for one-shot events.
    //    Filtered by -v/-vv/--quiet.
    let cli_layer = CliOutputLayer::new().with_filter(cli_filter.clone());

    // 3. CLI persistent spinners — one row per open span carrying `verb`.
    //    Same filter (so --quiet hides the live display too).
    let spinner_layer = SpinnerLayer::new().with_filter(cli_filter);

    tracing_subscriber::registry()
        .with(file_layer)
        .with(cli_layer)
        .with(spinner_layer)
        .init();

    guard
}
