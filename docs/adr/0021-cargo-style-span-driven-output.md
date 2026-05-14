# ADR-0021: Cargo-Style Span-Driven CLI Output

## Status

Accepted

## Context

The original CLI output was structured around `indicatif::MultiProgress` with
pre-allocated `ProgressBar` instances passed through the call stack:

- One *flow* spinner for the overall scheduler (`[*]` prefix, magenta).
- One *plan* spinner per package in flight (`[plan-name]` prefix, green).
- A `ProgressBar` field threaded through `PipelineContext`, `Pipeline`,
  `Forger::build`, `DriveOptions`, etc., updated via `set_message` at each
  stage transition.
- One-shot operational events emitted via `tracing::info!` with a custom
  `CliOutputLayer` that formatted lines as `INFO [plan] stage started …`.

This design accumulated friction along several axes:

1. **Concurrency mismatch.** Pre-allocating "one spinner per plan" stopped
   modeling reality once we introduced per-source parallel fetch, batch-wide
   CAS lookups, and split-part seal/deploy phases. Units of work appeared and
   disappeared at finer granularity than the pre-allocated slot table.

2. **Plumbing cost.** Every long-running operation that wanted a live row
   required a `ProgressBar` reference plumbed through every function that
   called it. New concurrency primitives meant editing call sites all the way
   from `bin/wright.rs` down to leaf modules.

3. **Output style drift.** The `INFO [plan] stage started (strict isolation)`
   grammar mixed log-level prefix (`INFO`), bracket-scope (`[plan]`), action
   verb (`started`), and metadata (`(strict isolation)`) into one line that
   was hard to scan and full of jargon the user could not act on.

4. **Failure path was broken.** The terminal failure line emitted only the
   `"Command failed"` message; the underlying error went to a structured
   `error = %e` field that the CLI layer ignored. The hint pointed to
   `wright.log` but `tracing_appender::rolling::daily` actually writes
   `wright.log.YYYY-MM-DD`.

5. **Misaligned with cargo.** Operators familiar with `cargo` and `pnpm`
   expected verb-aligned scrolling history plus a fixed live-status area.
   Wright's hybrid felt unfamiliar.

## Decision

Adopt a Cargo-style, span-driven output architecture.

### Three tracing layers, one source of truth

A single `tracing::info!` / `tracing::info_span!` call drives all surfaces:

| Layer | Purpose |
|-------|---------|
| `CliOutputLayer` | Scrolling 12-col right-aligned verb lines for one-shot events |
| `SpinnerLayer` | Persistent live rows attached to open spans carrying `verb` + `target` |
| `tracing_subscriber::fmt::layer().json()` | Structured JSON file log |

### Cargo-style verb lexicon and tense rules

Every user-facing message uses a 12-character verb followed by a target string,
governed by three rules:

- **Rule A** — Task initiation uses present-participle (`Compiling foo`),
  serving as both start announcement and in-progress indicator.
- **Rule B** — Intermediate completion is silent (no `Compiled foo` after
  every step); the next stage's `-ing` line is the implicit success signal.
- **Rule C** — Workflow-terminal verbs `Finished` / `Failed` / `Aborted`
  appear only at the boundary of an entire command.

Failure verbs `Failed` and `Aborted` render bold red instead of bold green;
all other action verbs are bold green. `warning:` and `error:` (cargo-style
zero-indent prefixes) handle warning/error events without verbs.

### Spans as the persistent-row contract

The persistent live area is no longer pre-allocated. Each in-flight unit of
work opens a tracing span with `verb` and `target` fields; `SpinnerLayer`
hooks `on_new_span` / `on_record` / `on_close` to attach, update, and remove
`ProgressBar` instances. Byte progress (HTTP downloads, git fetch) records
`bytes_done` / `bytes_total` on the span and the layer auto-swaps the row's
style from spinner to download bar.

Callers use the `cli_span!` macro:

```rust
let _s = crate::cli_span!("Compiling", "{}", plan_name);
// ... work; spinner is live this whole scope
// drop here clears the row
```

The macro returns a `tracing::Span` (not `EnteredSpan`) because `EnteredSpan`
is `!Send` and breaks `tokio::spawn`'d futures. The SpinnerLayer only relies
on the open/close lifecycle, not on "entered" state.

### Cargo / anyhow-style multi-line failure reports

Terminal failures (`cli_failed!`) bypass the verb-aligned layout and use a
zero-indent paragraph format:

```
error: task 'bison' failed

Caused by:
    0: forge bison
    1: failed to clean forge directory /var/tmp/wright/workshop/bison-3.8.2
    2: Device or resource busy (os error 16)

See /var/log/wright/wright.log.2026-05-15 for the full trace.
```

`util::logging::split_error_chain` splits the flattened `WrightError` Display
string on `": "` and drops `WrightError`-variant prefixes (`forge error`,
`deploy error`, …). The trailing hint points at the correct rolling-daily
filename via `today_log_path`.

### Companion architectural changes

The output refactor surfaced (and depended on) three correctness fixes that
this ADR also records:

1. **`force_clean_dir`** — `Forger::clean()` now scans `/proc/self/mounts` for
   stale overlay mounts under the build root, lazy-unmounts each
   (`MNT_DETACH`, deepest first), and retries `remove_dir_all`. Replaces the
   previous one-shot `tokio::fs::remove_dir_all` that died with `EBUSY` when a
   prior crashed run left an overlay mounted.

2. **CPU-sized compile pool** — `compile_lock` migrated from
   `Arc<Mutex<()>>` (binary, one compile at a time globally) to
   `Arc<Semaphore>` sized at `total_cpus`. Each `compile` stage calls
   `acquire_many(compile_cpu_count)`. Single-core compiles no longer block
   the whole batch on a multi-core box. `configure_lock` migrated to
   `Semaphore::new(1)` for type consistency; behavior preserved.

3. **Per-source parallel fetch** — `Forger::fetch` was a serial `for` loop;
   now builds one future per source and runs them through
   `futures_util::future::try_join_all`. A new `Forger.network_pool:
   Arc<Semaphore>` caps total concurrent downloads at
   `config.network.max_concurrent_downloads` (default 8). HTTP downloads are
   wrapped in `tokio::task::spawn_blocking` since `reqwest::blocking` would
   otherwise block the async runtime worker.

## Consequences

### Positive

- New concurrency primitives get a live row for free — open a span, the row
  appears; drop it, the row disappears. No threading of `Option<ProgressBar>`.
- The scrolling history and the live display share one rendering primitive
  (tracing spans/events) instead of two parallel code paths.
- File logging is automatic — every `cli_action!` and `cli_span!` writes a
  structured record without extra plumbing.
- Output matches operator expectations from `cargo` and `pnpm`. Failure
  reports follow anyhow's convention for error chains.
- Misleading `warning:` lines for self-healing paths (e.g. stale overlay mount
  cleaned up automatically) are demoted to `debug!`. The file log keeps the
  trail; the user is not alerted to non-issues.

### Negative / Tradeoffs

- `cli_span!` callers must bind the span by name (`let _s = ...`) and must
  NOT call `.entered()`. This is documented in the macro comment but is an
  easy footgun for new contributors.
- The `WrightError` cause-chain reconstruction is string-based
  (`split_error_chain` splits on `": "`). If a file path or error message
  happens to contain `": "`, the split mis-attributes segments. A proper fix
  requires `WrightError` to implement `std::error::Error::source()` properly,
  which means restructuring every variant to hold a typed cause instead of a
  `String`. Deferred — not blocking the rest of the work.
- The CLI verbosity filter applies to both the scroll layer and the spinner
  layer (they share the `EnvFilter`). `--quiet` hides the spinners entirely,
  which is intentional but worth knowing.

### Removed APIs (greenfield rewrite — no backwards compatibility)

The following helpers in `util::progress` are deleted:

- `new_plan_pipeline_spinner`
- `new_source_spinner`
- `new_source_transfer_bar` (replaced by spans + `record_bytes`)
- `new_build_flow_spinner`
- `ProgressBarGuard`
- `set_source_bytes`
- `set_source_git_objects`

The following fields are removed from struct surfaces:

- `PipelineContext.progress: Option<ProgressBar>`
- `Pipeline.progress`
- `DriveOptions.flow_progress`

The `cli_info!` macro is renamed to `cli_action!(verb, ...)` to make verb
selection a required argument at every callsite.

## References

- [tracing-output-design](../dev/tracing-output-design.md) — operational guide
  for authors adding new CLI messages.
- [ADR-0018](0018-unified-cli-porcelain-plumbing.md) — porcelain/plumbing
  split (independent concern, both still hold).
- [ADR-0019](0019-cas-delivery-recovery.md) — delivery FSM (`Sealing` and
  `Deploying` spans wrap the per-batch transitions defined there).
