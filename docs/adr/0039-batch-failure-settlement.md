# ADR-0039: Batch failure settlement instead of fail-fast

## Status

Accepted

## Context

Install and build execute dependency levels as batches of parallel tasks
(see [Batch Processing](../explanation/batch-processing.md)). The original
schedulers were fail-fast: the first failing task aborted the workflow
while its siblings were still running. On `wright install` the early
return dropped the remaining `JoinHandle`s, so sibling builds detached and
kept running while the CLI printed the terminal failure report and rolled
the delivery transaction back underneath them; only the first error was
ever reported. On `wright build` the batch was awaited in full, but still
only the first error surfaced.

Two forces pull against fail-fast here:

- Tasks within a batch have no inter-dependencies by construction, so a
  sibling's remaining work is never made worthless by the failure.
- Forge tasks are long (compiles run tens of minutes) and their output is
  reusable through forge checkpoints and the CAS; killing healthy
  in-flight compiles discards real work, and reporting only the first
  error forces fix-one-rerun-hit-the-next loops.

## Decision

Batch failure semantics are cargo-style **settle-then-report**:

1. A failing task never interrupts its siblings. Every task in the batch
   runs to completion — or to its own failure — on its own.
2. Each failure is announced the moment it happens as a single
   `error: task '<name>' failed: <cause>` line that points at the
   per-stage log file.
3. When no task is left running, the batch settles: zero failures lets the
   workflow continue; exactly one failure produces the pre-existing
   terminal failure report unchanged; more than one produces an aggregated
   report (`error: N tasks failed in batch i/j`) whose `Caused by:` list
   enumerates every failed task.
4. A failed batch blocks the next batch and — for install — rolls the
   delivery transaction back: the batch stays the atomic unit of
   progression. Cancellation (Ctrl-C) still wins over failure collection:
   reaped tasks settle as a single "cancelled by user" outcome.

Both batch drivers (`drive_batches` for `wright build`, the install loop
in `execute_install`) implement these semantics through the shared
`BatchFailures` settlement type.

## Alternatives

- **Keep fail-fast.** Rejected: wastes healthy in-flight compiles, hides
  all but the first error, and — on install — leaked detached tasks whose
  builds raced the rollback.
- **Fail-fast with cooperative cancellation of siblings.** Rejected:
  sibling work within a batch is independent and remains valuable, so
  cancelling it saves no dependency-critical work and only lengthens
  reruns. Cargo makes the same call for its `rustc` jobs.
- **Continue into later batches despite failures (`--keep-going`).**
  Rejected: later batches depend on the failed one by construction, so
  their results would be untrustworthy, and the extra mode would
  complicate the transaction model for little gain. It can be revisited
  as an opt-in flag if a real use case appears.

## Consequences

- Positive: no healthy compile is killed by a sibling's failure; every
  failure of the batch is visible in one terminal report; the install
  path no longer detaches still-running build tasks; the delivery
  rollback runs only after no build task is left, removing the race
  between rollback and in-flight builds.
- Positive: a single failure keeps the exact pre-existing terminal
  report, so tooling that matches on it keeps working.
- Negative: a doomed batch runs to completion before the workflow aborts,
  so wall-clock time to the final error can grow. This is accepted
  because the completed work is reusable on the rerun.
- The `(see log: …)` cause-chain splitting artifact is fixed as part of
  the same reporting rework.
