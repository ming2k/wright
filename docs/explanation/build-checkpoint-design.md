# Build Checkpoint Design

Build checkpoints exist so that Wright can safely resume a plan after a crash,
a network failure, a script error, or a human interrupt. The guarantee is
simple: **a stage is only considered complete after the stage script and its
hooks all finish successfully**. If a compile is killed halfway through, the
next run reruns the compile stage from the beginning.

## Stage Sentinels

`StageCheckpoint` (`src/builder/checkpoint.rs`) writes a sentinel file inside the
plan's `work_dir` only **after** the stage returns `Ok(())`:

```rust
self.run_ordered_stage(stage_name).await?;

if checkpoint_enabled {
    self.checkpoint
        .mark_complete(stage_name, &self.plan_fingerprint);
}
```

The sentinel stores the plan fingerprint:

```
fingerprint=<sha256-of-plan-and-mvp>
```

`is_complete` returns `true` only when the sentinel exists **and** the stored
fingerprint matches the current plan. If the plan changes, old sentinels are
silently ignored and the stage reruns.

## Why an Interrupted Compile Is Not "Done"

The pipeline (`src/builder/lifecycle.rs`) never writes a sentinel for a stage
that fails, panics, or is killed:

- `run_ordered_stage` executes the stage script + pre/post hooks.
- If any step returns an error, the `?` operator short-circuits before
  `mark_complete`.
- If the process receives SIGKILL, the thread never reaches `mark_complete`.
- If Ctrl-C is pressed, the batch runner stops launching new plans but allows
the current stage to finish or fail on its own. The sentinel is still only
written on success.

Therefore `.wright-stage-compile` exists **if and only if** the compile stage
and its hooks ran to completion. There is no partial or in-progress sentinel.

## Content-Addressed Invalidation

Checkpoints are keyed by a content hash, not by time or run ID. This means:

- Re-running the exact same command after a failure reuses every stage that
  already succeeded.
- Editing the plan file changes the fingerprint, invalidates all sentinels,
  and forces a clean rebuild.

`invalidate_from` (`src/builder/checkpoint.rs`) removes a stage and every stage
after it in the canonical pipeline order. This is used when `staging/` is
recreated: earlier sentinels such as `prepare`, `configure`, and `compile` are
kept, but `staging` and later stages are cleared.

## Source Tree Reuse Boundary

Before lifecycle stages run, the builder decides whether the extracted source
tree in `work/` can be reused.

`.build_key` (`src/builder/mod.rs`) is written only after `fetch`, `verify`,
and `extract` all succeed and `.extracted` is committed. It covers plan name,
version, release, declared sources, extraction targets, lifecycle scripts, and
executors.

If the build key still matches, the next run keeps `work/` and skips the
built-in source stages. If the build key differs, `work/` is wiped and the
sources are fetched again.

## Failure Boundaries

Build resume is **not** a global transaction rollback. It is an optimistic
skip-list:

- A successful stage gets a sentinel.
- A failed stage gets nothing.
- The next run skips what it can prove succeeded and reruns everything else.

This is intentionally simple. Build scripts are expected to be idempotent or
to maintain their own incremental state (e.g., `make` inside `work/`). Wright
guarantees only the stage boundary: either the whole stage ran, or it did not.

## Test Verification

`tests/integration/build_test.rs` asserts this behavior:

- First build fails in `staging`.
- `.wright-stage-prepare` exists, `.wright-stage-staging` does not.
- Second build skips `prepare` but reruns `staging`.
