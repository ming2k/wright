# How to Resume a Failed Build

Wright automatically resumes builds from the last completed lifecycle stage.
No `--resume` flag is needed — re-running the same command checks builder
staging checkpoints and skips stages that already succeeded.

## Resume After a Failed Build

```bash
# First run — fails during compile:
wright apply curl --deps

# Re-run exactly the same command:
wright apply curl --deps
# Already-completed stages (fetch, verify, prepare, configure) are skipped;
# compilation and later stages resume from where they left off.
```

The builder records a `StagingCheckpoint` after each lifecycle stage finishes.
On re-invocation, the checkpoint compares the stored plan fingerprint against
the current plan; when they match, the stage is skipped.

## Start Fresh

Use `--clean` to remove the build workspace and start from scratch:

```bash
wright apply curl --deps --clean
```

Use `--force` to re-run all lifecycle stages even when stage sentinels exist:

```bash
wright apply curl --deps --force
```

## More Detail

See [Build Resume Model](../explanation/build-resume-model.md) for how
`source_dir`, `work/`, stage sentinels, `outputs/`, archives, `--clean`, and
`--force` interact.
