# Build Resume Model

Wright treats resume as a normal startup path, not as a separate recovery mode.
The same command should be safe to run again after a crash, an interrupt, a
network failure, or a plan script failure. The implementation is deliberately
layered so each layer owns the validity rules for the data it can actually
verify.

## Resume Contract

Resume is keyed by command intent, not by the most recent chronological run.
When the user repeats the same normalized command, Wright checks builder
staging checkpoints and skips lifecycle stages that already succeeded for the
current plan fingerprint.

This is the replacement for an explicit `--resume <id>` flow. The command line
itself is the resume selector.

## Staging Checkpoints

The builder records a `StagingCheckpoint` after each lifecycle stage completes.
The checkpoint stores:

- The plan fingerprint (content hash of the plan file and optional `mvp.toml`)
- A map of stage names to completion status

On re-invocation, the builder compares the current plan fingerprint against the
checkpoint. When they match, completed stages are skipped. When they differ, the
checkpoint is invalidated and all stages rerun.

Unlike the previous workflow-based resume model, there is no SQLite table
tracking high-level orchestration steps. The checkpoint file lives in the build
workspace and is the sole source of resume truth for a single plan build.

## Retry Path

The retry path for a repeated build command is:

1. Normalize user inputs and create the execution plan (dependency graph + batches).
2. For each plan in the build set, start a builder lifecycle.
3. The builder reads the existing `StagingCheckpoint` for that plan.
4. For each lifecycle stage:
   - If the checkpoint shows the stage as complete and the fingerprint matches,
     skip the stage (unless `--force` is passed).
   - Otherwise, run the stage.
   - On success, record the stage as complete in the checkpoint.
5. Continue until all batches finish or a stage fails.

For `wright apply`, this means an earlier installed wave remains installed if a
later wave fails. Re-running the same `apply` rebuilds and packages the
remaining work, then installs it.

## Resume Boundaries

Resume happens at lifecycle stage boundaries inside a single plan build:

- `fetch`/`verify` succeeded: the source cache and verified hash are reused.
- `prepare` succeeded: the extracted source tree in `work/` is reused.
- `configure` succeeded: the configuration sentinel prevents re-running `configure`.
- `compile` succeeded: compiler output inside `work/` is preserved.
- `check` succeeded: the check sentinel is present.
- `staging` succeeded: the staged output tree is present for packaging.

Downstream commands (`package`, `install`) rely on artifact existence rather
than checkpoint state:

- `package` skips re-slicing when the expected archives already exist and their
  recorded hashes match.
- `install` skips reinstalling when the target part is already present and up to
date.

## Interrupts

Human interruption is part of the resume contract. Ctrl-C sends a cancellation
signal to the batch runner. The runner stops launching new plans, waits for
already-running plans to return, and exits.

Completed stages remain recorded in each plan's `StagingCheckpoint`. Stages that
were never started have no checkpoint entry. Re-running the same command
therefore resumes from the last persisted stage boundary.

This is a stage-boundary guarantee, not an instruction-level checkpoint inside a
build script. A build still relies on its own build key, extracted source tree,
stage sentinels, and output checks to decide what can be reused after
interruption.

## Artifact Layers

Resume state exists at two layers:

**Staging Checkpoint** (per plan):
- stores completed lifecycle stages
- indexed by plan fingerprint
- invalidated automatically when the plan changes

**Build artifacts** (per plan):
- downloaded or copied sources in `source_dir`
- the materialized source tree in `work/`
- successful lifecycle stages via `.wright-stage-<name>` files in `work/`
- sliced output directories under `outputs/`
- archives under `parts_dir`

These are not interchangeable. The staging checkpoint says whether a stage
finished. Build artifacts say whether local build outputs are still valid.
Keeping those questions separate prevents stale checkpoint entries from
becoming a global build cache.

## Build Key Boundary

The build key is the contract for reusing `work/`. It covers the plan name,
version, release, declared sources, source extraction targets, lifecycle script
content, and lifecycle executors.

Wright commits `.build_key` as soon as `work/` has been successfully
materialized and `.extracted` has been written. This is the important recovery
boundary: once sources are present for the current build key, later lifecycle
failures must not force a clean re-extraction on the next retry.

That means a first build can fail after `prepare`, `configure`, or `compile`,
and the next identical build can still reuse:

- the already extracted source tree
- incremental compiler output inside `work/`
- stage sentinels for lifecycle stages that completed successfully

If the build key changes, `work/` is not reused. Wright cleans it and
materializes sources again.

## Stage Sentinels

Stage sentinels are written only after a lifecycle stage and its hooks complete
successfully. A failed stage never receives a sentinel.

On a retry, Wright skips a stage when all of these are true:

- the build key still matches
- `work/` is present and extracted
- `.wright-stage-<name>` exists
- the user did not pass `--force`
- the user is running the normal full pipeline, not `--stage`

`staging` is special because its output depends on the
freshly recreated `staging/` tree. When `staging/` is recreated, Wright removes
its sentinel while preserving earlier stage sentinels such as `prepare`,
`configure`, and `compile`.

## Fresh Starts

The recovery controls intentionally operate at different layers:

- `--clean` removes the per-plan build workspace, including `work/` and all
  stage sentinels. It keeps the source cache.
- `--force` ignores output/archive skip checks and ignores stage sentinels while
  keeping reusable source material unless combined with `--clean`.

This split gives operators precise control. Use the smallest reset that matches
the suspected bad state.

## Failure Boundaries

Build resume is not global transaction rollback. If `wright apply` installs an
earlier dependency wave and a later wave fails, the installed earlier wave
remains applied. The next identical `apply` resumes from the remaining failed
or pending work.

That behavior is intentional. Wright favors convergence: advance safely, record
where it stopped, and make the next run continue from a verified boundary.
