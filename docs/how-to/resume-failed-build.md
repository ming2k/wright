# How to Resume a Failed Build

Wright V4 uses **content-addressed workflows** — no `--resume` flag needed.
Re-running the same command automatically skips already-succeeded steps and
retries only the failed or pending work.

## Resume After a Failed Build

```bash
# First run — fails on package 15 of 30:
wright apply curl --deps

# Re-run exactly the same command:
wright apply curl --deps
# Already-succeeded steps are skipped; failed/pending steps are resumed.
```

The workflow model creates a content-addressed plan from your inputs (targets,
flags, dependency scope). On re-invocation, the same inputs produce the same
workflow ID, so the runner finds the prior state and picks up where it left off.

## Start Fresh

Use `--fresh` to discard all prior workflow state and start from scratch:

```bash
wright apply curl --deps --fresh
```

## How It Works

1. **Workflow ID**: Computed as `SHA-256(kind, canonical_json(inputs))`.
   Same command = same ID every time.

2. **Step State**: Each build/package/install step is a row in `workflow_steps`
   with a status (`pending`, `running`, `succeeded`, `failed`). Step IDs are
   also content-addressed from their inputs, making them deterministic.

3. **Resume**: On each run, the scheduler loads the current status of every
   step. Steps with `succeeded` status are skipped. Steps with `failed` or
   `pending` status are re-attempted.

4. **Crash Recovery**: If Wright is killed mid-run, any `running` steps are
   reset to `pending` on the next invocation (the database lock guarantees no
   other process owns them).

5. **Retry Limit**: Failed steps are retried up to 3 times across all runs.
   After that, they are considered permanently failed.

## Inspect Runs

```bash
# List recent runs
wright runs list

# Inspect a failed run
wright runs show <run-id>

# Clean up old runs
wright runs gc --days 30
```
