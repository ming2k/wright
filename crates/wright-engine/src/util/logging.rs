use crate::error::BatchFailures;
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Style};
use std::collections::HashMap;
use std::fmt;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
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
pub static USE_COLOR: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal());

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
/// The report is driven by the real error chain (`std::error::Error::source`),
/// never by re-parsing Display output: every chain node contributes at most
/// one entry, so messages that themselves contain `": "` (sqlx's
/// `(code: 8) …`, TOML parse errors, `(see log: …)` suffixes) can no longer
/// be shredded into fake, numbered "causes".
///
/// A node's own message is recovered by stripping its source's Display off
/// the end of its own — `"{msg}: {source}"` is this workspace's nesting
/// format (see `WrightError::context`). Nodes whose own text is empty
/// (`#[error(transparent)]` wrappers) or exactly a variant label
/// (`forge error`, `database error`, …) carry no information and are
/// skipped. Nodes left over from string-flattened construction get leading
/// variant labels stripped, but their text is otherwise shown whole —
/// never split.
///
/// Layout follows `cargo`: zero-indent `error:` headline, blank separator,
/// `Caused by:` block with 4-space indented unnumbered entries (multi-line
/// messages keep their internal alignment), blank separator, log-file hint.
///
/// Returns a `Vec<String>` so the caller can print each line through
/// `MULTI.println` — which serializes against active progress bars.
pub fn format_failure_report(
    err: &(dyn std::error::Error + 'static),
    log_path: &std::path::Path,
) -> Vec<String> {
    let mut chain = error_chain_messages(err);
    let head = if chain.is_empty() {
        "command failed".to_string()
    } else {
        chain.remove(0)
    };

    let mut lines = Vec::new();
    lines.push(format_error(&head));
    if !chain.is_empty() {
        lines.push(String::new());
        lines.push("Caused by:".to_string());
        push_indented(&mut lines, 4, &chain);
    }
    lines.push(String::new());
    lines.push(format!("See {} for the full trace.", log_path.display()));
    lines
}

/// Flatten an error's chain into a single line — the same per-node
/// segmentation as the terminal failure report, rejoined with `": "`.
/// Used for the immediate per-task failure notice.
pub fn flatten_error_causes(err: &(dyn std::error::Error + 'static)) -> String {
    error_chain_messages(err).join(": ")
}

/// Emit the immediate one-line notice for a task that failed while batch
/// siblings are still running. Cargo-style: the failing task's siblings
/// keep running, and every failure is settled together once the batch
/// completes (see [`BatchFailures`]). Callers skip this notice for the
/// failure that empties a batch — the terminal failure report follows
/// immediately and would print the same failure twice.
pub fn report_task_failure(task: &str, panicked: bool, error: &(dyn std::error::Error + 'static)) {
    let causes = flatten_error_causes(error);
    let outcome = if panicked { "panicked" } else { "failed" };
    if causes.is_empty() {
        tracing::error!(event = "task.failed", task_name = %task, "task '{task}' {outcome}");
    } else {
        tracing::error!(
            event = "task.failed",
            task_name = %task,
            "task '{task}' {outcome}: {causes}"
        );
    }
}

/// Build the settlement report for a batch that finished with more than one
/// failed task — the multi-failure counterpart of [`format_failure_report`].
/// Each entry carries one task's headline with its own cause chain nested
/// underneath, so a long parallel run re-lists every failure that may have
/// scrolled by since its immediate notice.
pub fn format_batch_failure_report(
    failures: &BatchFailures,
    log_path: &std::path::Path,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format_error(&failures.to_string()));
    lines.push(String::new());
    lines.push("Caused by:".to_string());
    for failure in &failures.failures {
        lines.push(format!("    {}", failure.headline()));
        push_indented(&mut lines, 8, &error_chain_messages(&failure.error));
    }
    lines.push(String::new());
    lines.push(format!("See {} for the full trace.", log_path.display()));
    lines
}

/// Push each message under an `indent`-space margin, line by line.
/// Multi-line messages keep their internal alignment; blank inner lines
/// stay blank (no trailing whitespace).
fn push_indented(lines: &mut Vec<String>, indent: usize, msgs: &[String]) {
    let pad = " ".repeat(indent);
    for msg in msgs {
        for l in msg.lines() {
            lines.push(format!("{}{}", pad, l).trim_end().to_string());
        }
    }
}

/// Flatten `err`'s `source()` chain into the per-node messages worth
/// showing: headline first, then one entry per cause. Transparent wrappers
/// and bare variant labels are dropped; no message is ever split.
fn error_chain_messages(err: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut msgs = Vec::new();
    let mut cur = Some(err);
    while let Some(node) = cur {
        let msg = node_own_message(node);
        if !msg.is_empty() && !is_variant_label(&msg) {
            msgs.push(msg);
        }
        cur = node.source();
    }
    msgs
}

/// A node's own message: its Display with its source's Display stripped off
/// the end (`"{own}: {source}"` nesting), then any leading variant labels
/// removed. Empty for transparent wrappers, whose Display equals their
/// source's. When the source's text is not a suffix (source embedded
/// mid-message), the node's Display is kept whole — the source still
/// appears as its own entry, so nothing is lost.
fn node_own_message(err: &(dyn std::error::Error + 'static)) -> String {
    let full = err.to_string();
    let own = match err.source() {
        Some(src) => {
            let child = src.to_string();
            if full == child {
                String::new()
            } else if !child.is_empty() && full.ends_with(&child) {
                full[..full.len() - child.len()]
                    .trim_end_matches([' ', ':'])
                    .to_string()
            } else {
                full
            }
        }
        None => full,
    };
    strip_variant_labels(&own)
}

/// Remove leading `"<label>: "` prefixes left over from string-flattened
/// error construction (`VariantError(format!("…: {}", e))`). Labels are an
/// exact, closed set drawn from this workspace's error enums, so this can
/// never eat real message text.
fn strip_variant_labels(msg: &str) -> String {
    let mut s = msg;
    for _ in 0..4 {
        match s.split_once(": ") {
            Some((head, rest)) if is_variant_label(head) && !rest.is_empty() => s = rest,
            _ => break,
        }
    }
    s.to_string()
}

/// The bare labels emitted by this workspace's error enums
/// (`#[error("<label>: …")]` variants), as produced by `WrightError`,
/// `StateError`, `PartError`, `PlanError`, and `ModelError`.
fn is_variant_label(seg: &str) -> bool {
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
            | "access denied"
            | "lock error"
            | "version error"
            | "dependency error"
            | "part not found"
            | "ambiguous target"
            | "part already deployed"
            | "upgrade error"
            | "script error"
            | "validation error"
            | "isolation error"
            | "network error"
            | "TOML deserialization error"
            | "SQLite error"
            | "cache error"
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
    use crate::error::WrightError;

    // Stand-ins for sqlx-style foreign errors whose Display nests the
    // source's text: "error returned from database: (code: 8) …".
    #[derive(Debug, thiserror::Error)]
    enum DbError {
        #[error("error returned from database: {0}")]
        Returned(#[source] DbFailure),
    }

    #[derive(Debug, thiserror::Error)]
    #[error("(code: 8) attempt to write a readonly database")]
    struct DbFailure;

    // Mirror of `WrightError::context` / `StateError::SqliteError` shapes.
    #[derive(Debug, thiserror::Error)]
    enum TestError {
        #[error("{msg}: {source}")]
        Context {
            msg: String,
            #[source]
            source: Box<dyn std::error::Error + Send + Sync>,
        },
        #[error("SQLite error: {0}")]
        Sqlite(#[from] DbError),
    }

    fn context(
        msg: &str,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> TestError {
        TestError::Context {
            msg: msg.to_string(),
            source: source.into(),
        }
    }

    #[test]
    fn failure_report_layers_structured_chain_without_numbers() {
        // The readonly-database scenario: a `": "`-carrying sqlx message
        // must survive as whole entries, never split into fake causes.
        let err = context(
            "failed to begin delivery transaction",
            TestError::Sqlite(DbError::Returned(DbFailure)),
        );
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        assert_eq!(lines.len(), 7, "lines: {lines:?}");
        assert!(
            lines[0].contains("error:")
                && lines[0].contains("failed to begin delivery transaction"),
            "headline: {}",
            lines[0]
        );
        assert!(!lines[0].contains("SQLite"), "headline: {}", lines[0]);
        assert!(lines[1].is_empty());
        assert_eq!(lines[2], "Caused by:");
        assert_eq!(lines[3], "    error returned from database");
        assert_eq!(
            lines[4],
            "    (code: 8) attempt to write a readonly database"
        );
        assert!(lines[5].is_empty());
        assert!(lines[6].starts_with("See "));
        for line in &lines {
            assert!(
                !line.trim_start().starts_with("0:") && !line.trim_start().starts_with("1:"),
                "cause entries are never numbered: {line}"
            );
        }
    }

    #[test]
    fn failure_report_skips_transparent_wrappers() {
        // `#[error(transparent)] Model(#[from] ModelError)` adds no text of
        // its own; the ModelError label is dropped too.
        let err = WrightError::Model(wright_model::ModelError::ValidationError(
            "bad version spec".to_string(),
        ));
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        assert_eq!(lines.len(), 3, "lines: {lines:?}");
        assert!(
            lines[0].contains("bad version spec"),
            "headline: {}",
            lines[0]
        );
        assert!(
            !lines[0].contains("validation error"),
            "headline: {}",
            lines[0]
        );
    }

    #[test]
    fn failure_report_never_splits_flattened_nodes() {
        // A legacy string-flattened error is shown whole: one headline, no
        // invented causes, mid-string labels untouched.
        let err = WrightError::ForgeError(
            "task 'b' failed: forge error: error returned from database: (code: 8) attempt to write a readonly database".to_string(),
        );
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        assert_eq!(lines.len(), 3, "lines: {lines:?}");
        assert!(
            lines[0].contains(
                "task 'b' failed: forge error: error returned from database: (code: 8) attempt to write a readonly database"
            ),
            "headline: {}",
            lines[0]
        );
    }

    #[test]
    fn failure_report_keeps_multiline_cause_alignment() {
        #[derive(Debug, thiserror::Error)]
        #[error("TOML parse error at line 3, column 5\n  |\n3 | bad = [\n  |     ^")]
        struct TomlLike;

        let err = context("failed to parse plan file", TomlLike);
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        let expected: Vec<String> = [
            "error: failed to parse plan file",
            "",
            "Caused by:",
            "    TOML parse error at line 3, column 5",
            "      |",
            "    3 | bad = [",
            "      |     ^",
            "",
            "See /log for the full trace.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(
            lines[0].contains("error:") && lines[0].contains("failed to parse plan file"),
            "headline: {}",
            lines[0]
        );
        assert_eq!(&lines[1..], &expected[1..]);
    }

    #[test]
    fn failure_report_hint_suffix_survives_whole() {
        // "(hint: …)" used to need a dedicated re-join heuristic; with no
        // splitting at all it simply stays put.
        let err = WrightError::AccessDenied(
            "permission denied for lock file /var/lib/wright/lock/cmd-wright.lock".to_string(),
        );
        let path = std::path::PathBuf::from("/log");
        let lines = format_failure_report(&err, &path);
        assert_eq!(lines.len(), 3, "lines: {lines:?}");
        assert!(
            lines[0].contains(
                "permission denied for lock file /var/lib/wright/lock/cmd-wright.lock. (hint: try running with sudo)"
            ),
            "headline: {}",
            lines[0]
        );
    }

    #[test]
    fn flatten_error_causes_rejoins_chain_nodes() {
        let flat = WrightError::ForgeError(
            "stage 'compile' failed with exit code 1 (see log: /tmp/compile.log)".to_string(),
        );
        assert_eq!(
            flatten_error_causes(&flat),
            "stage 'compile' failed with exit code 1 (see log: /tmp/compile.log)"
        );

        let layered = context(
            "failed to begin delivery transaction",
            TestError::Sqlite(DbError::Returned(DbFailure)),
        );
        assert_eq!(
            flatten_error_causes(&layered),
            "failed to begin delivery transaction: error returned from database: \
             (code: 8) attempt to write a readonly database"
        );
    }

    #[test]
    fn batch_failure_report_nests_each_tasks_causes() {
        use crate::error::{BatchFailures, TaskFailure};
        let batch = BatchFailures {
            batch_num: 1,
            total_batches: 2,
            failures: vec![
                TaskFailure::failed(
                    "igc",
                    WrightError::ForgeError(
                        "stage 'compile' failed with exit code 1 (see log: /tmp/compile.log)"
                            .to_string(),
                    ),
                ),
                TaskFailure::panicked("neenee", WrightError::ForgeError("task panicked".into())),
            ],
        };
        let path = std::path::PathBuf::from("/log");
        let lines = format_batch_failure_report(&batch, &path);
        // headline / blank / Caused by / task / cause / task / cause / blank / see ...
        assert_eq!(lines.len(), 9, "lines: {lines:?}");
        assert!(lines[0].contains("2 tasks failed in batch 1/2"));
        assert_eq!(lines[2], "Caused by:");
        assert_eq!(lines[3], "    task 'igc' failed");
        assert_eq!(
            lines[4],
            "        stage 'compile' failed with exit code 1 (see log: /tmp/compile.log)"
        );
        assert_eq!(lines[5], "    task 'neenee' panicked");
        assert_eq!(lines[6], "        task panicked");
        assert!(lines[8].starts_with("See "));
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

/// Number of WARN events shown on the CLI during this command run. Used to
/// qualify the terminal `Finished` line so a warning mid-run doesn't leave
/// the user guessing whether it mattered.
static CLI_WARN_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Warnings displayed so far in this command run.
pub fn cli_warn_count() -> usize {
    CLI_WARN_COUNT.load(Ordering::Relaxed)
}

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

impl Default for CliOutputLayer {
    fn default() -> Self {
        Self::new()
    }
}

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
        if level == tracing::Level::WARN {
            CLI_WARN_COUNT.fetch_add(1, Ordering::Relaxed);
        }

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
            crate::util::progress::term_println(&line);
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
///     row from a spinner to a download bar
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
