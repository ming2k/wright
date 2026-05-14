# Logging

Where Wright writes logs and how to control verbosity. For the design rationale
see [tracing-output-design](../dev/tracing-output-design.md) and
[ADR-0021](../adr/0021-cargo-style-span-driven-output.md).

## Output Surfaces

| Surface | What | Where |
|---------|------|-------|
| Scrolling CLI lines | Cargo-style 12-col verb-aligned actions and `warning:` / `error:` messages | stderr |
| Persistent CLI rows | Live spinners for in-flight work; one row per open span | bottom of terminal (stderr) |
| Diagnostic file log | Structured JSON, full event/field context | `<logs_dir>/wright.log.YYYY-MM-DD` |
| Build tool log | Per-stage stdout/stderr from build commands (`make`, etc.) | `<build_dir>/<plan>-<version>/logs/<stage>.log` |

## Verbosity

CLI verbosity and file-log verbosity are independent.

| Flag | CLI level | File level |
|------|-----------|------------|
| (none) | `info` | `debug` |
| `-v` | `debug` | `debug` |
| `-vv` | `trace` | `debug` |
| `--quiet` | `warn` | `debug` |

The file log always defaults to `debug` so a post-mortem trace is available
without re-running with `-v`. Override with `WRIGHT_LOG=<level>` for the
session (e.g. `WRIGHT_LOG=trace wright install foo`).

## Log File Location

| Field | Default | Notes |
|-------|---------|-------|
| `logs_dir` (root) | `/var/log/wright` | system-wide install |
| `logs_dir` (user) | `~/.local/state/wright` | non-root install |
| Filename | `wright.log.YYYY-MM-DD` | rolling daily via `tracing_appender::rolling::daily` |

A failed CLI command points at today's file in the `Failed` line's
`See <path> for the full trace` hint.

## Build Tool Logs

Per-stage logs land at `<build_dir>/<plan>-<version>/logs/<stage>.log`:

| Stage | File |
|-------|------|
| `prepare` | `prepare.log` |
| `configure` | `configure.log` |
| `compile` | `compile.log` |
| `check` | `check.log` |
| `staging` | `staging.log` |

Each run **recreates** the per-stage directory, so build output is always fresh
for the most recent attempt. Older runs' build logs are not preserved — use the
diagnostic file log for cross-run history.

## Field Conventions

Events and spans use the following well-known fields. The CLI layers (scroll
and spinner) read them; the file layer logs them all.

| Field | Type | Purpose |
|-------|------|---------|
| `verb` | string | Drives the 12-col scrolling verb column and the spinner row prefix |
| `target` | string | The thing being acted on (plan, file, batch) — span field |
| `plan_name` | string | Plan scope |
| `stage_name` | string | Pipeline stage |
| `event` | string | Stable event slug for aggregation (e.g. `forge.started`) |
| `error` | display | Underlying error — folded into the body on WARN/ERROR levels |
| `bytes_done` / `bytes_total` | u64 | Recorded on spans to swap the row to a download bar |
| `elapsed_secs` | f64 | Duration of completed stage |
| `trace_id` | string | Per-command UUID, propagated across the call tree |
