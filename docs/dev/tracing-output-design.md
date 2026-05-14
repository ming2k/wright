# CLI Output & Tracing Design

This page defines how Wright produces user-facing terminal output and structured
diagnostic logs. Use it when adding or changing any operator-facing message.

For the rationale behind the architecture see
[ADR-0021](../adr/0021-cargo-style-span-driven-output.md).

## Architecture: Three Tracing Layers

Wright uses a single source of truth — `tracing` events and spans — fanned out
through three `tracing_subscriber::Layer` instances:

| Layer | Purpose | Surface |
|-------|---------|---------|
| `CliOutputLayer` | Scrolling Cargo-style action lines for one-shot events | stderr (via `MultiProgress.println`) |
| `SpinnerLayer` | Persistent in-flight rows driven by open span lifecycle | bottom of terminal (`MultiProgress`) |
| File layer (`tracing_subscriber::fmt::layer().json()`) | Structured JSON for diagnostics | `logs_dir/wright.log.YYYY-MM-DD` |

The same `tracing::info!` / `info_span!` call drives all three. No layer is
authoritative; each subscribes to the fields it cares about.

### Span vs Event

| You want… | Use |
|-----------|-----|
| A one-shot scrolling line ("we just started X") | `cli_action!(verb, fmt, args...)` |
| A persistent live row that exists for a scope ("X is happening") | `cli_span!(verb, fmt, args...)` |
| A warning/error scroll line | `cli_warn!` / `cli_error!` |
| The terminal failure line (red, with cause chain) | `cli_failed!` |
| User-interrupted termination | `cli_aborted!` |
| File-only diagnostic (not user-facing) | `tracing::debug!` or `info!` without `verb` |

`cli_span!` also emits the matching `cli_action!` scroll line at span open, so
the user sees both the scrolled history entry *and* the live row. Rule B
(below) means there is no closing scroll line — the span just disappears.

---

## 1. Scrolling Output (CliOutputLayer)

### Layout

Every event with a `verb` field renders as a Cargo-style action line:

```
   Planning 1 package: linux-lts
    Fetching linux-7.0.6.tar.xz (linux-lts)
  Extracting linux-7.0.6.tar.xz (linux-lts)
    Building linux-lts
    Skipping fetch, verify, extract (linux-lts)
   Preparing linux-lts
 Configuring linux-lts
   Compiling linux-lts
    Checking linux-lts
     Staging linux-lts
     Sealing linux-lts
   Deploying 1 part
    Finished install in 2m45s
```

- **12-character right-aligned verb column**, single space, then the target.
- Standard verbs render in **bold green**. The terminal failure verbs `Failed`
  and `Aborted` render in **bold red**.
- `warning:` (bold yellow) and `error:` (bold red) are zero-indent prefixes,
  emitted when the event has no `verb` field but reaches WARN/ERROR level.

### Verb Lexicon

Every verb is ≤12 characters and ends in `-ing` (Rule A: present-participle
announces task initiation; the spinner row is the in-progress indicator) or is
one of the terminal Rule C verbs `Finished` / `Failed` / `Aborted`.

| Category | Verbs |
|----------|-------|
| Planning / scheduling | `Planning`, `Building`, `Batch` |
| Source acquisition | `Fetching`, `Verifying`, `Extracting` |
| Forge stages | `Preparing`, `Configuring`, `Compiling`, `Checking`, `Staging` |
| Per-package post-forge | `Sealing` |
| Delivery | `Deploying`, `Upgrading`, `Installing`, `Removing`, `Providing`, `Cascading` |
| Diagnostic commands | `Checking`, `Linting`, `Verifying` |
| Cache / skip | `Skipping`, `Cached` |
| Workflow end | `Finished` (success), `Failed` (error), `Aborted` (user interrupt) |

When a stage name is custom (user-defined in a plan), `forge::logging::stage_verb`
maps the stage to its gerund; unknown stages fall back to `Running`.

### Three Tense Rules

These come from the project style guide and apply to every CLI message.

| Rule | What | Why |
|------|------|-----|
| **A — Initiation** | Use `-ing` when work begins. The same line acts as the start announcement *and* the in-progress indicator. | One line per logical operation. No "starting X" / "running X" duplication. |
| **B — Implicit silence** | Do **not** emit a `Compiled` / `Fetched` / `Done` line when a step completes. The next stage's `-ing` line is the success signal. | Past-tense completion spam clutters history and competes visually with start lines. |
| **C — Terminal completion** | Use past-tense `Finished` / `Failed` / `Aborted` **only** for the whole workflow (install, build, doctor, etc.). | These are the punctuation marks of a session, not per-step status. |

Concrete consequence: the per-stage `info!(event = "stage.completed")` events
that fire when each forge stage ends carry no `verb` field. They go to the file
log only — the user sees the row disappear and that is the success signal.

---

## 2. Persistent Live Rows (SpinnerLayer)

### Convention

A persistent row exists for the duration of any open span that carries the
fields `verb` and `target`. The layer hooks:

| `Layer` callback | Effect |
|------------------|--------|
| `on_new_span` | If `verb` is set: attach a `ProgressBar` to the global `MultiProgress` |
| `on_record` | If `bytes_done` / `bytes_total` are recorded: update position/length and swap to the byte-progress bar style on first total |
| `on_close` | `finish_and_clear` the bar; remove from tracking |

Rows are ordered by span creation time (oldest first). Visible columns: animated
spinner glyph, verb in bold green, target, dimmed elapsed time.

### Macro: `cli_span!`

```rust
let _s = crate::cli_span!("Compiling", "{}", plan_name);
// ... do work; spinner is live this whole scope
// drop happens here; spinner clears
```

The macro returns a `tracing::Span` that must be bound by name (`let _s = ...`)
so it stays alive for the scope. Do **not** call `.entered()` — `EnteredSpan`
is `!Send` and the future will fail to `tokio::spawn`. The SpinnerLayer only
uses `on_new_span` / `on_close`, so whether the span is "entered" is irrelevant
to the display.

### Byte Progress

For HTTP / git downloads, record byte counts on the span and the row auto-swaps
from a spinner to a download bar:

```rust
let span = crate::cli_span!("Fetching", "{} ({})", filename, plan);
loop {
    let n = response.read(&mut buf)?;
    if n == 0 { break; }
    downloaded += n as u64;
    crate::util::progress::record_bytes(&span, downloaded, total_size);
}
```

The bar template is:

```
⠁ Fetching libaom-3.13.1.tar.gz (linux-lts) 4s [###---------]  12.3 MB / 100.1 MB
```

### Why Spans, Not Pre-Allocated Bars

The previous implementation passed `Option<ProgressBar>` through the call stack
(`Forger::build` → `PipelineContext` → `Pipeline`) and pre-allocated one bar
per known unit of work. That broke as soon as concurrency stopped matching the
pre-allocation — for example, per-source parallel fetch within a plan, or
batch-wide CAS lookups happening between forge and seal.

Spans solve this because units of work *are* spans: open one when you start, it
becomes visible automatically; drop it and it disappears. No plumbing. The
renderer doesn't need to know the plan structure ahead of time.

---

## 3. Failure Reporting

Terminal failures use the `cli_failed!` macro, which fires a
`tracing::error!(verb = "Failed", ...)` event. The wright binary entry point
(`src/bin/wright.rs`) intercepts the failure path:

1. `MULTI.clear()` wipes active spinners so they don't bleed into the report.
2. `suppress_cli_output()` blocks any in-flight cleanup events from racing past.
3. `tracing::error!` records the structured event (file log).
4. `format_failure_report(&err, &log_path)` builds the multi-line block.
5. Each line is printed through `MULTI.println` so it serializes with any
   surviving progress bars.

### Layout

Cargo / anyhow style — zero-indent, paragraph format, no verb-column alignment:

```
error: task 'bison' failed

Caused by:
    0: forge bison
    1: failed to clean forge directory /var/tmp/wright/workshop/bison-3.8.2
    2: Device or resource busy (os error 16)

See /var/log/wright/wright.log.2026-05-15 for the full trace.
```

The cause chain is recovered by `split_error_chain` — it splits the
`WrightError` Display string on `": "` and drops `WrightError`-variant prefixes
(`forge error`, `deploy error`, `database error`, …). Single-cause failures
render their cause unnumbered; multi-cause use `0:`, `1:`, `2:` indices.

The trailing `See <path>` line points at the rolling daily log file produced by
`tracing_appender::rolling::daily` — the path includes today's `YYYY-MM-DD`
suffix.

---

## 4. Verbosity Levels

| Level | Flag | EnvFilter | Default file-log? | CLI output? |
|-------|------|-----------|-------------------|-------------|
| `INFO` | (default) | `info` | yes | events with `verb`; spans with `verb` |
| `DEBUG` | `-v` | `debug` | yes | also non-verb diagnostic events |
| `TRACE` | `-vv` | `trace` | yes | every event |
| WARN/ERROR | always | always | yes | `warning:` / `error:` lines |
| `--quiet` | | `warn` | yes (unchanged) | warnings and errors only |

The file layer's verbosity is independent of the CLI flag: it defaults to
`debug` and can be overridden by `WRIGHT_LOG=trace` (etc.). The diagnostic log
is comprehensive by design — operators should never need `-v` retroactively
after a failure.

---

## 5. Color & TTY Detection

Color is controlled by `util::logging::USE_COLOR`, a `LazyLock<bool>`:

```rust
USE_COLOR = NO_COLOR is unset AND stderr.is_terminal()
```

Follows [no-color.org](https://no-color.org). When false, all the verb-line
helpers (`format_action`, `format_warn`, `format_error`) emit plain text that
remains greppable.

---

## 6. File Handler (Diagnostic Logs)

Structured JSON, one record per event, written to a rolling daily file:

```
logs_dir/wright.log.YYYY-MM-DD
```

Each record includes `timestamp`, `level`, `target`, `trace_id`, and every
field declared on the event. The schema is stable for log aggregation
(ELK, Loki, Datadog).

### Example

```json
{
  "timestamp": "2026-05-15T12:00:00Z",
  "level": "INFO",
  "verb": "Compiling",
  "target": "openssl",
  "plan_name": "openssl",
  "stage_name": "compile",
  "trace_id": "abc123…"
}
```

### Style Constraints for Structured Logs

- **No sensitive data** (PII, tokens, secrets).
- **Provide context** — `plan_name`, `stage_name`, `error` fields. Single-word
  events (`"Error"`, `"Done"`) are useless to aggregators.
- **Tense matches the CLI rules** — `-ing` for in-progress, past for completed,
  silent for intermediate completion.

---

## 7. Migration Notes for Authors

If you are touching CLI output:

1. **Adding a new action line?** Pick a verb from the lexicon, use
   `cli_action!`. Keep it ≤12 chars, `-ing` ending, follow Rule B for completion.
2. **Adding a long-running operation?** Use `cli_span!` instead — gives the user
   a live row for the duration without you plumbing a `ProgressBar`.
3. **Adding a warning?** Default to `cli_warn!`. If the situation is self-healing
   (we recovered, the user can't do anything), demote to `tracing::debug!` —
   warnings should be user-actionable.
4. **Adding an error?** `cli_error!` for non-terminal errors; `cli_failed!` only
   for the workflow's final failure line (it gets the multi-line Caused-by
   treatment).
5. **Passing `ProgressBar` around?** Don't. Open a span instead. The legacy
   `progress: Option<ProgressBar>` plumbing was deleted; do not reintroduce it.
