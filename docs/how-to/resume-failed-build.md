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

Wright records workflow state for the normalized inputs: targets, flags, and
dependency scope. On re-invocation, the same inputs produce the same workflow
ID, so the runner finds the prior step state and picks up where it left off.

## Start Fresh

Use `--invalidate` to discard all prior workflow state and start from scratch:

```bash
wright apply curl --deps --invalidate
```

## More Detail

See [Build Resume Model](../explanation/build-resume-model.md) for the database
workflow state behind resume and for how `source_dir`, `work/`, stage
sentinels, `outputs/`, archives, `--invalidate`, `--clean`, and `--force`
interact.
