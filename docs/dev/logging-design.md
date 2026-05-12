# Logging Design

This page defines the log system design for Wright's operator-facing CLI output.
Use it when adding or changing `INFO`/`WARN`/`ERROR` messages.

## Goals

- Optimize for terminal scanning during long-running operations.
- Make the current unit of work obvious without reading previous lines.
- Keep `INFO` logs stable enough that docs and troubleshooting guides can cite
  them.

## Event Ownership

Each layer owns a different kind of message:

- Scheduler: capacity, batch boundaries, resume state, final summary.
- Plan execution: plan start/done, stage start/done, plan-local skips.
- Artifact emission: produced part paths and other actionable outputs.
- Transactions: install/upgrade/remove events for the system root.

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

Plan lines use a stable scope prefix and mark lifecycle boundaries:

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
  plan a clear lifecycle boundary.

### Plan lifecycle tracking

When multiple plans run concurrently in a dependency batch, their stage lines
interleave. Without explicit plan boundaries the operator cannot determine
when one plan's lifecycle ends and the next begins. Wright inserts three
lifecycle events per plan:

- **forge started** — emitted when the lifecycle pipeline begins executing
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
same lifecycle events apply. In a two-batch build the output might interleave:

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

Plan lifecycle boundaries are emitted by `Forger::build()` in
`src/forge/mod.rs`. The message text is constructed by pure formatting
helpers in `src/forge/logging.rs`:

| Call site | Helper | Example output |
|---|---|---|
| Before `pipeline.run()` | `forge_started(name)` | `[zlib] forge started` |
| After `pipeline.run()` success | `forge_finished(name)` | `[zlib] forge done` |
| After `pipeline.run()` error | `forge_failed(name)` | `[zlib] forge failed` |

The stage-level messages (`stage_started`, `stage_finished`,
`stage_skipped`) are emitted inside `LifecyclePipeline::run()` in
`src/forge/pipeline.rs`. Because these calls happen within the
`Forger::build()` scope, they naturally nest between the `forge started`
and `forge done` lines.

### Resident progress bar

Wright maintains a persistent spinner at the bottom of the terminal that tracks
a plan through its full lifecycle — from the delivery-level operation down to
each micro stage inside the forge pipeline:

```text
⠂ [linux] [00:02:13] compiling
⠂ [zlib] [00:00:05] fetching sources
⠂ [openssl] [00:00:47] configuring
```

**Template**: `{spinner:.green} [{plan-name}] [{elapsed_precise}] {current-stage}`

The spinner is "resident" — it remains visible while the plan executes and
updates its message to reflect whatever stage is currently in progress.  When
multiple plans run concurrently in a dependency batch, each gets its own
spinner line managed by `indicatif::MultiProgress`.

**Stages are not hardcoded.** The spinner's `{current-stage}` is driven
directly by the plan's lifecycle stages (from `plan.toml`) and the built-in
pipeline stages.  During built-in stages (fetch / verify / extract) the
messages are fixed, but during pipeline execution the progress bar displays
the actual stage name from the manifest:

| Phase | Spinner message origin |
|---|---|
| Built-in (fetch) | `"fetching sources"` (fixed) |
| Built-in (verify) | `"verifying checksums"` (fixed) |
| Built-in (extract) | `"extracting sources"` (fixed) |
| Pipeline stage | Raw stage name from `plan.toml` lifecycle (e.g. `"prepare"`, `"compile"`) |

The elapsed timer resets for each phase so operators can see per-stage
duration at a glance.

#### Implementation

The spinner is created by `new_plan_lifecycle_spinner()` in
`src/util/progress.rs` and managed inside `Forger::build()` in
`src/forge/mod.rs`.  A `ProgressBarGuard` RAII wrapper ensures
`finish_and_clear()` runs on any return path (success, error, or `?`
propagation).

```rust
// src/util/progress.rs — spinner creation
pub fn new_plan_lifecycle_spinner(plan_name: &str) -> ProgressBar {
    let pb = MULTI.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} [{prefix}] [{elapsed_precise}] {msg}")
            .expect("valid plan lifecycle template"),
    );
    pb.set_prefix(format!("[{}]", plan_name));
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

// src/forge/mod.rs — lifecycle orchestration
let lifecycle_bar = new_plan_lifecycle_spinner(&manifest.metadata.name);
let _guard = ProgressBarGuard(lifecycle_bar.clone());  // auto-finish on drop

lifecycle_bar.set_message("fetching sources".to_string());
self.fetch(manifest, plan_dir).await?;

// … verify, extract, then pipeline …
pipeline::LifecyclePipeline::new(LifecycleContext {
    progress: Some(lifecycle_bar),  // pipeline updates msg per stage
    // …
})?;
pipeline.run().await?;
// _guard drops here → spinner finished
```

The `LifecyclePipeline` updates the spinner message inside
`run_stage_with_hooks_in_target()` (`src/forge/pipeline.rs`) before each
stage executes, using the stage name verbatim from the manifest.

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

## Style Constraints

- One line should communicate one new fact.
- Default to sentence fragments, not full prose paragraphs.
- Keep status verbs consistent: `started`, `done`, `skipped`, `packed`, `installed`, `failed`.
- Prefer scopes over repeated nouns. `[linux] prepare started` is better than `Plan linux: starting stage prepare`.
- Put durations at the end of successful completion lines.
- Put explanatory detail in `DEBUG` when it is not needed to operate the command.
- Keep human-facing counts and labels stable across runs unless behavior changed.

## Verbosity Split

- `INFO`: operator timeline.
- `DEBUG`: extra diagnostics, internal decisions, low-level timing.
- `TRACE`: deep implementation detail.
- `WARN`/`ERROR`: abnormal conditions, failures, or degraded behavior.

Every long-running happy-path step should still have a compact `INFO` line even
if richer `DEBUG` output exists.
