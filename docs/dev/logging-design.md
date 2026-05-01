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

Plan lines use a stable scope prefix:

```text
INFO [linux] build started
INFO [linux] build done
INFO [linux] skipped: parts already exist (use --force to rebuild)
```

Rules:

- Use `[plan-name]` as the first token.
- Prefer short verbs: `started`, `done`, `skipped`, `packed`.
- Do not repeat `plan`, `task`, or `INFO` in the message body.

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
