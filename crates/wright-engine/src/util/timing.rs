//! Unified workflow step timing — the single timing facility for wright.
//!
//! One [`WorkflowTiming`] registry per command run. Each named step is
//! measured by a [`StepGuard`] (RAII): the elapsed time is recorded when
//! the guard drops — as successful only if [`StepGuard::success`] was
//! called, otherwise as failed. A step that bails early via `?` therefore
//! still has its duration counted; a failed step never loses its timing.
//!
//! At the end of the run — success or failure — the aggregated per-step
//! durations plus the wall-clock total are rendered as a Cargo-style
//! block and emitted through tracing, so the report lands on the CLI and
//! in the daily log file alike.
//!
//! The registry is `Arc`-shared and cheap to clone: future extensions
//! (e.g. per-pipeline forge timing recorded from spawned build tasks)
//! can contribute steps to the same report without extra plumbing.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::util::logging::VERB_WIDTH;

/// Compact human-readable duration: `<1s` → `Nms`, `<60s` → `1.2s`,
/// otherwise `1m23s`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{}ms", (secs * 1000.0).round() as u64)
    } else if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let total = secs.round() as u64;
        format!("{}m{:02}s", total / 60, total % 60)
    }
}

/// Step timers recorded for one command run. Clone-cheap (`Arc` inside);
/// every clone records into the same registry.
pub struct WorkflowTiming {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    started: Instant,
    steps: Vec<StepRecord>,
}

struct StepRecord {
    name: String,
    elapsed: Duration,
    ok: bool,
}

impl Default for WorkflowTiming {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for WorkflowTiming {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl WorkflowTiming {
    /// Start the registry clock.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                started: Instant::now(),
                steps: Vec::new(),
            })),
        }
    }

    /// Start measuring a named step. The returned guard records the step
    /// when it drops: successful only if [`StepGuard::success`] was called,
    /// failed otherwise — so an early error return (`?`) still counts.
    pub fn step(&self, name: impl Into<String>) -> StepGuard {
        StepGuard {
            timing: self.clone(),
            name: name.into(),
            start: Instant::now(),
            ok: false,
        }
    }

    /// Record a step whose duration was measured elsewhere.
    pub fn record(&self, name: impl Into<String>, elapsed: Duration, ok: bool) {
        self.lock().steps.push(StepRecord {
            name: name.into(),
            elapsed,
            ok,
        });
    }

    /// Wall-clock time since [`WorkflowTiming::new`].
    pub fn elapsed(&self) -> Duration {
        self.lock().started.elapsed()
    }

    /// Snapshot the registry: repeated step names are aggregated (summed
    /// duration, run count, failed if any run failed) in first-seen order.
    pub fn summary(&self) -> TimingSummary {
        let inner = self.lock();
        let mut steps: Vec<StepSummary> = Vec::new();
        for record in &inner.steps {
            match steps.iter_mut().find(|s| s.name == record.name) {
                Some(s) => {
                    s.runs += 1;
                    s.elapsed += record.elapsed;
                    s.ok &= record.ok;
                }
                None => steps.push(StepSummary {
                    name: record.name.clone(),
                    runs: 1,
                    elapsed: record.elapsed,
                    ok: record.ok,
                }),
            }
        }
        TimingSummary {
            total: inner.started.elapsed(),
            steps,
        }
    }

    /// Emit the end-of-run timing report through tracing: a Cargo-style
    /// `Timing` block on the CLI plus structured fields for the file log.
    ///
    /// Success renders at INFO (suppressed under `--quiet`, matching the
    /// `Finished` line); failure renders at WARN so it is always shown.
    pub fn log_report(&self, workflow: &str, ok: bool, quiet: bool) {
        let summary = self.summary();
        let block = summary.render(workflow, !ok);
        let total_secs = summary.total.as_secs_f64();
        let steps = summary.compact();
        if ok {
            if !quiet {
                tracing::info!(
                    verb = "Timing",
                    event = "workflow.timing",
                    workflow = %workflow,
                    ok = true,
                    total_secs,
                    steps = %steps,
                    "{}",
                    block,
                );
            }
        } else {
            tracing::warn!(
                verb = "Timing",
                event = "workflow.timing",
                workflow = %workflow,
                ok = false,
                total_secs,
                steps = %steps,
                "{}",
                block,
            );
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned timing mutex must never crash the exit path.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// RAII step measurement. Records into the parent registry on drop.
///
/// The step is recorded as **failed unless [`StepGuard::success`] was
/// called** — deliberately, so that `?` early returns mark the step
/// failed without any extra code on the error path.
pub struct StepGuard {
    timing: WorkflowTiming,
    name: String,
    start: Instant,
    ok: bool,
}

impl StepGuard {
    /// Mark the step successful and record it immediately.
    pub fn success(mut self) {
        self.ok = true;
        // Drop runs here, recording with ok = true.
    }
}

impl Drop for StepGuard {
    fn drop(&mut self) {
        self.timing.record(
            std::mem::take(&mut self.name),
            self.start.elapsed(),
            self.ok,
        );
    }
}

/// One aggregated step row: summed over `runs` same-named recordings.
pub struct StepSummary {
    pub name: String,
    pub runs: usize,
    pub elapsed: Duration,
    pub ok: bool,
}

impl StepSummary {
    /// Row label: the step name, plus `×N` when the step ran repeatedly
    /// (e.g. once per dependency batch).
    fn label(&self) -> String {
        if self.runs > 1 {
            format!("{} ×{}", self.name, self.runs)
        } else {
            self.name.clone()
        }
    }
}

/// Frozen end-of-run snapshot, ready to render.
pub struct TimingSummary {
    /// Wall-clock total of the whole run (includes time outside steps).
    pub total: Duration,
    /// Aggregated steps in first-seen order.
    pub steps: Vec<StepSummary>,
}

impl TimingSummary {
    /// Render the Cargo-style report block: a header line (rendered after
    /// the verb column by the CLI layer) followed by one row per step and
    /// a `total` row, each continuation line indented to sit under the
    /// header. Failed steps keep their duration and carry a `(failed)`
    /// marker; a failed run marks the header with `(failed)` too.
    pub fn render(&self, workflow: &str, failed: bool) -> String {
        let mut out = String::new();
        if failed {
            let _ = write!(out, "{workflow} step timing (failed):");
        } else {
            let _ = write!(out, "{workflow} step timing:");
        }

        let rows: Vec<(String, String, bool)> = self
            .steps
            .iter()
            .map(|s| (s.label(), format_duration(s.elapsed), s.ok))
            .collect();
        let total_label = "total";
        let total_dur = format_duration(self.total);
        let name_width = rows
            .iter()
            .map(|(l, _, _)| l.len())
            .max()
            .unwrap_or(0)
            .max(total_label.len());
        let dur_width = rows
            .iter()
            .map(|(_, d, _)| d.len())
            .max()
            .unwrap_or(0)
            .max(total_dur.len());

        // Continuation lines align under the message column, i.e. past the
        // right-aligned verb column plus its separating space.
        let indent = " ".repeat(VERB_WIDTH + 1);
        for (label, dur, ok) in &rows {
            let marker = if *ok { "" } else { " (failed)" };
            let _ = write!(
                out,
                "\n{indent}{label:<name_width$}  {dur:>dur_width$}{marker}"
            );
        }
        let _ = write!(
            out,
            "\n{indent}{total_label:<name_width$}  {total_dur:>dur_width$}"
        );
        out
    }

    /// Compact one-line form for the structured file log, e.g.
    /// `resolve=0.4s,forge=12.2s(!)`.
    pub fn compact(&self) -> String {
        self.steps
            .iter()
            .map(|s| {
                let mut entry = format!("{}={}", s.label(), format_duration(s.elapsed));
                if !s.ok {
                    entry.push_str("(!)");
                }
                entry
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn format_duration_chooses_unit() {
        assert_eq!(format_duration(Duration::from_millis(50)), "50ms");
        assert_eq!(format_duration(Duration::from_secs_f64(4.6)), "4.6s");
        assert_eq!(format_duration(Duration::from_secs(124)), "2m04s");
    }

    #[test]
    fn dropped_guard_records_failed_unless_success() {
        let timing = WorkflowTiming::new();
        {
            let _g = timing.step("forge");
            sleep(Duration::from_millis(5));
        }
        {
            let g = timing.step("seal");
            g.success();
        }
        let summary = timing.summary();
        assert_eq!(summary.steps.len(), 2);
        assert!(!summary.steps[0].ok, "unmarked drop must record failed");
        assert!(summary.steps[0].elapsed >= Duration::from_millis(5));
        assert!(summary.steps[1].ok);
    }

    #[test]
    fn repeated_step_names_aggregate_in_first_seen_order() {
        let timing = WorkflowTiming::new();
        timing.record("forge", Duration::from_secs(2), true);
        timing.record("seal", Duration::from_secs(1), true);
        timing.record("forge", Duration::from_secs(3), false);
        let summary = timing.summary();
        assert_eq!(summary.steps.len(), 2);
        assert_eq!(summary.steps[0].name, "forge");
        assert_eq!(summary.steps[0].runs, 2);
        assert_eq!(summary.steps[0].elapsed, Duration::from_secs(5));
        assert!(!summary.steps[0].ok, "any failed run fails the row");
        assert_eq!(summary.steps[1].name, "seal");
    }

    #[test]
    fn render_marks_failed_step_and_includes_total() {
        let timing = WorkflowTiming::new();
        timing.record("resolve", Duration::from_millis(400), true);
        timing.record("forge", Duration::from_millis(12200), false);
        let block = timing.summary().render("install", true);
        assert!(block.starts_with("install step timing (failed):"));
        assert!(block.contains("resolve"));
        assert!(block.contains("12.2s (failed)"));
        assert!(block.contains("total"));
    }

    #[test]
    fn render_aggregated_rows_show_run_count() {
        let timing = WorkflowTiming::new();
        timing.record("forge", Duration::from_secs(1), true);
        timing.record("forge", Duration::from_secs(1), true);
        let block = timing.summary().render("install", false);
        assert!(block.contains("forge ×2"));
        assert!(block.starts_with("install step timing:"));
    }

    #[test]
    fn compact_marks_failed_steps() {
        let timing = WorkflowTiming::new();
        timing.record("resolve", Duration::from_millis(400), true);
        timing.record("forge", Duration::from_secs(12), false);
        let compact = timing.summary().compact();
        assert_eq!(compact, "resolve=400ms,forge=12.0s(!)");
    }

    #[test]
    fn clones_share_one_registry() {
        let timing = WorkflowTiming::new();
        let clone = timing.clone();
        clone.record("deploy", Duration::from_secs(1), true);
        assert_eq!(timing.summary().steps.len(), 1);
    }
}
