# How to Resume a Failed Build

When a large cascade build fails partway through, use `--resume` to continue without re-building parts that already succeeded.

## Resume After a Failed Build

```bash
# First run — fails on package 15 of 30:
wright resolve pcre2 --rdeps --depth=0 | wright build --force
# Output: Build session: a1b2c3... (resume with: --resume a1b2c3...)

# Resume — skips the 14 already-completed packages:
wright resolve pcre2 --rdeps --depth=0 | wright build --resume
```

`--resume` tracks progress in a build session stored in the database. Each successfully built part is recorded. On resume, those parts are skipped and the rest are rebuilt.

## Resume and Install

If you need to install the rebuilt outputs afterward, print the archive paths and feed them to `wright install`:

```bash
wright resolve pcre2 --rdeps --depth=0 | wright build --resume --print-parts | wright install
```

## Auto-Detect the Session

The session hash is deterministic — running the same `wright resolve | wright build` pipeline produces the same hash, so `--resume` auto-detects the session. You can also pass the hash explicitly:

```bash
wright resolve pcre2 --rdeps --depth=0 | wright build --resume a1b2c3...
```

Sessions are cleaned up automatically when all parts complete successfully.

## Resume a Failed Apply

`wright apply` can also resume, but its behavior is intentionally split across two layers:

- The live system state determines which batches are already converged.
- The execution session remembers which build tasks already finished, so a resumed apply does not rebuild archives that were completed before the failure.

Re-run the same apply request with `--resume`:

```bash
wright apply @base --deps --resume
```

Or pass the explicit hash printed on failure:

```bash
wright apply @base --deps --resume a1b2c3...
```

`apply --resume` must be used with the same targets and scope flags (`--deps/--rdeps/--match/--depth/--force`) as the original run.
