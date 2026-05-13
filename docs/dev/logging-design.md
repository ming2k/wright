# Logging Design

This page defines the log system design for Wright's operator-facing CLI output.
Use it when adding or changing `INFO`/`WARN`/`ERROR` messages or resident progress
bars.

## Goals

- Optimize for terminal scanning during long-running operations.
- Make the current unit of work obvious without reading previous lines.
- Keep `INFO` logs stable enough that docs and troubleshooting guides can cite
  them.
- Provide two tiers of resident progress tracking: flow-level (`[*]`) and
  per-plan (`[plan-name]`), so the operator always sees what the scheduler and
  each concurrent forge are doing.

## Event Ownership

Each layer owns a different kind of message:

- Scheduler: capacity, batch boundaries, resume state, final summary.
- Plan execution: plan start/done, stage start/done, plan-local skips.
- Artifact emission: produced part paths and other actionable outputs.
- Transactions: install/upgrade/remove events for the system root.
- Resident spinners: persistent progress bars that track the build flow and
  per-plan forge pipelines.

Do not let multiple layers narrate the same transition.

## Message Grammar

### Batch lines

Batch lines summarize the next dependency wave:

```text
INFO Build batch 1/2: bootstrap gcc, build binutils.
INFO Apply batch 2/2: full rebuild gcc.
```

Rules:

- Use `Build` or `Apply` as the leading noun.
- Use `batch N/T`.
- Put the action list after the colon.
- Do not repeat task counts when the action list already shows the work.

### Plan lines

Plan lines use a stable scope prefix and mark pipeline boundaries:

```text
INFO [linux] forge started
INFO [linux] forge done
INFO [linux] forge failed
INFO [linux] skipped: parts already exist (use --rebuild to rebuild)
```

Rules:

- Use `[plan-name]` as the first token.
- Prefer short verbs: `started`, `done`, `skipped`, `packed`, `failed`.
- Do not repeat `plan`, `task`, or `INFO` in the message body.
- Open every forge with `[plan] forge started` and close it with
  `[plan] forge done` (or `[plan] forge failed` on error). This gives each
  plan a clear pipeline boundary.

### Plan pipeline tracking

When multiple plans run concurrently in a dependency batch, their stage lines
interleave. Without explicit plan boundaries the operator cannot determine
when one plan's pipeline ends and the next begins. Wright inserts three
pipeline events per plan:

- **forge started** — emitted when the pipeline begins executing
  (after fetch/verify/extract).
- **forge done** — emitted after the final stage commits successfully.
- **forge failed** — emitted when a stage fails, immediately before the error
  propagates.

A typical single-plan build produces:

```text
INFO [zlib] forge started
INFO [zlib] prepare started (strict isolation)
INFO [zlib] prepare done in 2.1s
INFO [zlib] compile started (strict isolation)
INFO [zlib] compile done in 12.3s
INFO [zlib] staging started (relaxed isolation)
INFO [zlib] staging done in 0.4s
INFO [zlib] forge done
```

For plans that reside below the top-level target in the dependency graph, the
same pipeline events apply. In a two-batch build the output might interleave:

```text
INFO Build batch 1/2: forge zlib.
INFO [zlib] forge started
INFO [zlib] compile done in 12.3s
INFO [zlib] forge done
INFO Build batch 2/2: forge openssl.
INFO [openssl] forge started
INFO [openssl] compile done in 45.1s
INFO [openssl] forge done
```

The `[plan-name]` scope prefix keeps every line attributable to the correct
plan regardless of interleaving.

#### Implementation

Plan pipeline boundaries are emitted by `Forger::build()` in
`src/forge/mod.rs`. The message text is constructed by pure formatting
helpers in `src/forge/logging.rs`:

| Call site | Helper | Example output |
|---|---|---|
| Before `pipeline.run()` | `forge_started(name)` | `[zlib] forge started` |
| After `pipeline.run()` success | `forge_finished(name)` | `[zlib] forge done` |
| After `pipeline.run()` error | `forge_failed(name)` | `[zlib] forge failed` |

The stage-level messages (`stage_started`, `stage_finished`,
`stage_skipped`) are emitted inside `Pipeline::run()` in
`src/forge/pipeline.rs`. Because these calls happen within the
`Forger::build()` scope, they naturally nest between the `forge started`
and `forge done` lines.

### Stage lines

Stage transitions are the main progress signal during builds:

```text
INFO [linux] prepare started (strict isolation)
INFO [linux] prepare done in 4.6s
INFO [linux] compile started (no isolation)
```

Rules:

- The start line names the isolation mode only once.
- The completion line carries the duration.
- Use the same stage name the manifest uses.
- Do not emit a second line from another layer that says the same stage began or ended.

### Artifact lines

Artifact lines surface paths only when the path is actionable:

```text
INFO [linux] packed /var/lib/wright/parts/linux-6.14.2-1-x86_64.wright.tar.zst
```

Rules:

- Include the full path for produced parts and log files.
- Do not attach incidental paths to ordinary progress messages.

## Resident Spinners

Wright uses a two-tier system of persistent progress bars (spinners) managed
by `indicatif::MultiProgress`.  They stay at the bottom of the terminal and
update their messages in real time without scrolling.  Every spinner is
wrapped in a `ProgressBarGuard` RAII guard so it is always cleaned up on any
exit path (success, error, `?` propagation, or panic).

### Flow spinner

The **flow spinner** tracks the overall build pipeline — from DAG resolution
through batch execution to completion.  It is the top-most spinner line:

```text
⠙ [*] [00:00:01] resolving build graph
⠙ [*] [00:00:05] batch 1/2: 2 plans
⠙ [*] [00:02:14] batch 2/2: 1 plan
⠙ [*] [00:03:45] complete
```

**Template**: `{spinner:.magenta} [*] [{elapsed_precise}] {msg}`

The `[*]` prefix (magenta) distinguishes it from per-plan `[plan-name]` lines
(green).  It is created by `new_build_flow_spinner()` in
`src/util/progress.rs` and managed at two call sites:

| Phase | Where | Message |
|---|---|---|
| DAG resolution | `execute_build()` in `src/operations/build.rs` | `"resolving build graph"` |
| Batch N/T | `drive_batches()` in `src/operations/drive.rs` | `"batch 2/5: 3 plans"` |
| Failure | `drive_batches()` on batch error | `"batch 2/5: aborted"` |
| Completion | `drive_batches()` after all batches | `"complete"` |

The spinner is passed from `execute_build()` into `drive_batches()` via
`DriveOptions::flow_progress`.

### Plan spinners

**Plan spinners** track individual forge pipelines.  Each concurrent plan in a
batch gets its own spinner line below the flow spinner:

```text
⠂ [zlib] [00:00:05] fetching sources
⠂ [zlib] [00:00:03] compiling
⠂ [openssl] [00:00:47] configuring
```

**Template**: `{spinner:.green} [{plan-name}] [{elapsed_precise}] {current-stage}`

Each spinner is created by `new_plan_pipeline_spinner()` in
`src/util/progress.rs` and managed inside `Forger::build()` in
`src/forge/mod.rs`.  The message is updated at each stage transition:

| Phase | Spinner message origin |
|---|---|
| Built-in (fetch) | `"fetching sources"` (fixed) |
| Built-in (verify) | `"verifying checksums"` (fixed) |
| Built-in (extract) / hardlink | `"hard-linking sources"` / `"extracting sources"` (fixed) |
| Pipeline stage | Raw stage name from `plan.toml` pipeline (e.g. `"prepare"`, `"compile"`) |

Stages are not hardcoded — the pipeline displays whatever stage names the plan
manifest defines.

### Two-tier example

A typical multi-plan build produces both tiers.  The flow spinner shows the
scheduler's current phase while plan spinners show what each plan is doing:

```text
⠙ [*] [00:00:01] resolving build graph
  ↓ (resolution completes)
⠙ [*] [00:00:05] batch 1/2: 2 plans
⠙ [zlib] [00:00:03] fetching sources
⠙ [zlib] [00:00:06] compiling
⠙ [ncurses] [00:00:02] extracting sources
  ↓ (zlib completes, ncurses continues, spinner line disappears for zlib)
⠙ [ncurses] [00:00:45] compiling
  ↓ (batch 1 completes)
⠙ [*] [00:02:14] batch 2/2: 1 plan
⠙ [openssl] [00:01:00] compiling
  ↓ (batch 2 completes)
⠙ [*] [00:03:45] complete
INFO all batches completed
```

When a plan finishes (or fails), its spinner is cleared via
`ProgressBarGuard::drop()`.  The flow spinner stays until the entire build
completes.

#### Implementation

Both spinner types use the global `MULTI` (`LazyLock<MultiProgress>`) in
`src/util/progress.rs`.  Text log lines are routed through
`MultiProgressWriter` so they are inserted above active spinners without
flickering.

```rust
// src/util/progress.rs — flow spinner
pub fn new_build_flow_spinner() -> ProgressBar {
    let pb = MULTI.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.magenta} [*] [{elapsed_precise}] {msg}")
            .expect("valid build flow template"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

// src/util/progress.rs — plan spinner
pub fn new_plan_pipeline_spinner(plan_name: &str) -> ProgressBar {
    let pb = MULTI.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} [{prefix}] [{elapsed_precise}] {msg}")
            .expect("valid plan pipeline template"),
    );
    pb.set_prefix(format!("[{}]", plan_name));
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}
```

## Style Constraints

- **User-centric Language:** Avoid exposing internal architectural or engineering jargon (e.g., "WAL", "filesystem transaction", "rollback journal") in `INFO` lines. Frame messages around the user's workflow (e.g., "Cleaning up interrupted installation from a previous run" instead of "Recovering unfinished filesystem transaction").
- One line should communicate one new fact.
- Default to sentence fragments, not full prose paragraphs.
- Keep status verbs consistent: `started`, `done`, `skipped`, `packed`, `installed`, `failed`.
- Prefer scopes over repeated nouns. `[linux] prepare started` is better than `Plan linux: starting stage prepare`.
- Put durations at the end of successful completion lines.
- Put explanatory detail in `DEBUG` when it is not needed to operate the command.
- Keep human-facing counts and labels stable across runs unless behavior changed.

## Verbosity Levels

Wright exposes four log levels via the `-v` / `-vv` flags (see
`src/bin/wright.rs`).  The default level is `INFO`.  Every long-running
happy-path step should still have a compact `INFO` line even if richer
`DEBUG`/`TRACE` output exists.

### INFO (default)

The operator timeline.  Shows what Wright is doing at a high level: batch
boundaries, forge pipeline events, stage start/done, artifact paths, and
transaction summaries.  These messages are stable enough to appear in
troubleshooting guides.

| Level | Flag | Filter | Timestamps | Target prefix |
|---|---|---|---|---|
| `INFO` | (default) | `info` | hidden | hidden |
| `DEBUG` | `-v` | `debug` | shown | shown |
| `TRACE` | `-vv` | `trace` | shown | shown |

### DEBUG (`-v`)

Internal decisions, low-level timing, and skipped-work explanations.  Use
`DEBUG` for information that helps developers or advanced operators understand
*why* Wright made a particular choice, but is not needed by the typical
operator.

Examples of what belongs in `DEBUG`:

```text
DEBUG Source tree unchanged (forge key match) — reusing layers/
DEBUG Sources already extracted — skipping fetch/verify/extract
DEBUG Stage prepare has empty script, skipping
DEBUG Built-in stage fetch is handled by Builder
DEBUG Skipping check stage due to --skip-check
DEBUG Using cached source: hello-1.0.tar.gz
DEBUG Computed hash: a1b2c3d4...
DEBUG Source hello-1.0.tar.gz already cached and verified
DEBUG Verified source: hello-1.0.tar.gz
DEBUG Dependency graph is acyclic.
DEBUG Running hook: ./pre-configure.sh
DEBUG Resetting checkpoint for stage: configure
DEBUG Clearing failed stage layer: /var/lib/wright/forge/.../layers/05-configure
DEBUG CAS miss: hello (not in store)
DEBUG CAS hit: hello (1234567 bytes)
DEBUG CAS: hello stored at /var/lib/wright/cas/h/hello.hash.zst
DEBUG Plan linux up-to-date
DEBUG Isolation child exited with: ExitCode(0)
```

Rules for `DEBUG`:

- Explain skipped work so operators know nothing is broken.
- Log forge key matches, hash computations, and cache hits/misses.
- Report hook invocations (`pre-*`, `post-*` scripts).
- Report internal state transitions (checkpoint rewinds, layer mounts).
- Include relevant identifiers (plan name, stage name, hash prefix) so lines
  are self-describing.

### TRACE (`-vv`)

Deep implementation detail.  Use `TRACE` for information that is only useful
when debugging the resolver, CAS, or pipeline internals.  These lines are
highly verbose and should never appear in operator-facing docs.

Examples of what belongs in `TRACE`:

```text
TRACE zlib depth 0 enqueued
TRACE openssl depth 1 enqueued
TRACE pcre2 depth 2 enqueued
TRACE hello closure_fp=a1b2c3d4
TRACE hello:bootstrap fp=e5f6a7b8
TRACE hello CAS check key=c9d0e1f2
TRACE hello origin=LinkDependency
```

Rules for `TRACE`:

- Log fine-grained DAG traversal (every plan enqueued, with depth).
- Log fingerprint computation intermediates (closure fingerprints, bootstrap
  fingerprints, CAS check keys).
- Log per-file operations and mount details.
- Do not emit `TRACE` in hot loops — these lines should be sparse enough that
  `-vv` output remains scannable.

### WARN / ERROR

Abnormal conditions, failures, or degraded behavior.  These are always shown
regardless of verbosity setting.

- `WARN`: non-fatal issues (e.g., "Failed to write forge key", "Failed to
  clean up scratch directory").
- `ERROR`: fatal failures that abort the current operation (e.g., "batch 1/2
  failed", transaction failures).
