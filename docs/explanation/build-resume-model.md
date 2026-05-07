# Build Resume Model

Wright treats resume as a normal startup path, not as a separate recovery mode.
The same command should be safe to run again after a crash, an interrupt, a
network failure, or a plan script failure. The implementation is deliberately
layered so each layer owns the validity rules for the data it can actually
verify.

## Resume Contract

Resume is keyed by command intent, not by the most recent chronological run.
When the user repeats the same normalized command, Wright derives the same
workflow ID and reuses the step state for that workflow. Succeeded steps are
left alone; failed, pending, or orphaned running steps become candidates for
retry.

This is the replacement for an explicit `--resume <id>` flow. The command line
itself is the resume selector.

## Database Workflow State

The system database stores enough workflow state to answer one question: where
did this command stop last time?

In this context, a workflow is the durable identity of a normalized command
intent. It is not one process execution and not one historical attempt. A run
is only an in-process attempt to drive that workflow. Wright does not persist
run history; it persists active resume state. This distinction is what makes
resume automatic without turning the workflow database into an audit log.

| Table | Resume role |
|-------|-------------|
| `workflows` | Stores active content-addressed workflow identities for incomplete work. |
| `workflow_steps` | Stores active resume state for incomplete workflows: dependencies, status, attempts, successful outputs, and small failure diagnostics. |

On startup, the command rebuilds the current step graph from the current inputs
and upserts it into the database. Existing step rows are not overwritten, which
is what preserves successful work across attempts. The runner then resets stale
`running` steps to `pending`, loads step statuses and outputs, and schedules
only the work whose dependencies have succeeded.

When a workflow succeeds, Wright deletes its active `workflows` row. The
`workflow_steps` rows cascade away with it. A later identical command rebuilds
the active workflow and relies on the build, package, install, and launch
layers to decide what can be skipped. Workflow step state therefore belongs to
unfinished work; it is not permanent history.

`workflow_steps.outputs_json` stores the successful step output as JSON. Its
schema is defined by the step kind, not by one global workflow schema. It is
machine state for downstream steps and later attempts, not a user-facing log. For
example, an install step can read an upstream package step's archive paths and
hashes without re-running the package step. The JSON should name durable
artifacts, hashes, counts, or other small facts; it should not embed build logs,
archive contents, or other bulk data.

`workflow_steps.failure_json` stores a small structured diagnostic for failed
steps. It is bounded metadata, not a log: reason, selected context, and a short
message. It must not contain stdout, stderr, log paths, or complete error
chains. The current command still prints the full immediate error, while full
build output remains in stage log files.

The important distinction is that the workflow rows are progress state. They
are not a universal build cache and they are not a serialized execution graph to
trust blindly. Every invocation rebuilds the graph in memory, then joins it with
the active persisted status rows.

## Step Output Examples

Row-level examples showing `workflow_steps.kind` beside its `outputs_json`:

```json
{
  "kind": "build_plan",
  "outputs_json": {
    "build_root": "/var/lib/wright/build/curl-8.7.1-r1",
    "output_dirs": [
      "/var/lib/wright/build/curl-8.7.1-r1/outputs/default"
    ]
  }
}
```

```json
{
  "kind": "package_plan",
  "outputs_json": {
    "archives": [
      {
        "name": "curl",
        "path": "/var/lib/wright/parts/curl-8.7.1-r1.x86_64.wright.tar.zst",
        "hash": "7a4a0d2e..."
      },
      {
        "name": "libcurl",
        "path": "/var/lib/wright/parts/libcurl-8.7.1-r1.x86_64.wright.tar.zst",
        "hash": "13f95c0b..."
      }
    ]
  }
}
```

```json
{
  "kind": "install_batch",
  "outputs_json": {
    "installed": [
      "curl",
      "libcurl"
    ]
  }
}
```

```json
{
  "kind": "extract_pack",
  "outputs_json": {
    "staging_dir": "/var/lib/wright/launch/staging/8fd4b61c...",
    "manifest_json": "{...}",
    "overlay_present": true
  }
}
```

## Failure Examples

Row-level examples showing `workflow_steps.kind` beside its `failure_json`:

```json
{
  "kind": "build_plan",
  "failure_json": {
    "reason": "stage_failed",
    "plan": "curl",
    "stage": "compile",
    "exit_status": 2,
    "message": "stage 'compile' failed with exit status 2"
  }
}
```

```json
{
  "kind": "package_plan",
  "failure_json": {
    "reason": "archive_missing",
    "plan": "curl",
    "message": "expected archive not produced"
  }
}
```

## Retry Path

The retry path for a repeated command is:

1. Normalize user inputs.
2. Derive the workflow ID from `kind` and canonical inputs.
3. Build the current workflow steps and dependency edges.
4. Insert any missing workflow and step rows.
5. Preserve `succeeded` step rows and their JSON outputs.
6. Reset stale `running` rows to `pending`.
7. Retry `failed` or `pending` steps whose dependencies have succeeded.
8. Record the new run as `succeeded`, `failed`, or `aborted`.
9. If the run succeeded, delete the workflow's active step state.

For `wright apply`, this means an earlier installed wave remains installed if a
later wave fails. Re-running the same `apply` resumes at the remaining failed or
pending build/package/install work instead of replaying the completed wave.

## Resume Boundaries

Workflow resume happens at step boundaries:

- `build_plan` succeeded: downstream `package_plan` can read its `outputs_json`
  and skip re-running that build step while the workflow is incomplete.
- `package_plan` succeeded: downstream `install_batch` can read archive paths
  and hashes from `outputs_json`.
- `install_batch` succeeded: later dependency waves can proceed without
  reinstalling that batch.

Inside a `build_plan`, resume is handled by the build layer, not the workflow
tables. If a lifecycle stage fails, the workflow step remains `failed`, but the
next attempt can reuse the build key, extracted `work/` tree, and
`.wright-stage-<name>` sentinels for stages that completed successfully.

Inside `package_plan`, archive existence and hashes decide whether packaging
work is reused. Inside `install_batch`, install transactions and installed part
hashes decide whether a part is already applied. The workflow database only
coordinates which high-level step should be attempted next.

## Interrupts

Human interruption is part of the resume contract. Ctrl-C sends a cancellation
signal to the workflow runner. The runner stops launching new steps, waits for
already-running steps to return, and leaves active workflow state for retry.

Completed steps stay `succeeded`. Steps that were never started remain
`pending`. If the process exits before a running step can finish and write its
final status, the next invocation resets stale `running` rows to `pending`
before scheduling work. Re-running the same command therefore resumes from the
last persisted step boundary.

This is a step-boundary guarantee, not an instruction-level checkpoint inside a
build script. A build step still relies on its own build key, extracted source
tree, stage sentinels, output checks, and archive checks to decide what can be
reused inside that step after interruption.

## Artifact Layers

While a workflow is incomplete, workflow state remembers which high-level steps
have succeeded for a command: build one plan, package one plan, install one
batch, extract one pack, or apply configuration. Re-running the same normalized
command produces the same workflow ID, so succeeded active steps can be skipped
and failed or pending steps can be retried. After success, that workflow step
state is deleted.

Build state remembers reusable work inside one plan build:

- downloaded or copied sources in `source_dir`
- the materialized source tree in `work/`
- successful lifecycle stages via `.wright-stage-<name>` files in `work/`
- sliced output directories under `outputs/`
- archives under `parts_dir`

These are not interchangeable. Workflow state says whether an orchestration
step finished. Build state says whether local build artifacts are still valid.
Keeping those questions separate prevents stale workflow rows from becoming a
global build cache.

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

- `--fresh` discards workflow rows for the command identity. It does not delete
  source caches, `work/`, stage sentinels, `outputs/`, or archives.
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
