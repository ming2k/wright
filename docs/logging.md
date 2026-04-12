# Logging

This page explains where Wright writes logs, how to control verbosity, and which
settings affect log locations. For the operator-facing log style and message
constraints, see [logging-design.md](logging-design.md).

## What Gets Logged

Wright has two logging channels:

- **CLI logs**: structured logs from `wright` system and build subcommands.
- **Build logs**: per-stage stdout/stderr captured for part builds.

They are configured separately.

## Log Format

CLI log lines are structured by ownership:

- Scheduler lines announce capacity, batches, resume state, and final summary.
- Plan lines use a stable `[plan]` scope for build and stage progress.
- Transaction lines report install/upgrade work.

Example:

```
INFO Build capacity: 16 parallel tasks on 16 CPU cores.
INFO Build batch 1/1: build go.
INFO [go] build started
INFO [go] configure started (strict isolation)
INFO [go] configure done in 2.3s
INFO Fetched go1.26.2.linux-amd64.tar.gz
INFO [go] packed /var/lib/wright/components/go-1.26.2-1-x86_64.wright.tar.zst
INFO [go] build done
INFO Installing go: 16681 files
INFO Installed go: 1.26.2-1
```

## CLI Verbosity

All `wright` subcommands use the same verbosity flags:

- `-v` enables debug logs.
- `-vv` enables trace logs.
- `--quiet` shows warnings and errors only.

Default level is `info`. Logs are printed to stderr by the CLI.

### Debug Timing for Install and Upgrade

At `-v`, install and upgrade flows emit phase timing at `DEBUG` level. This is
primarily intended for large packages with many files, where the slow phase may
be archive extraction, file metadata scanning, owner checks, filesystem writes,
or database updates.

Example:

```text
INFO Installing texlive-texmf: 252553 files
DEBUG install texlive-texmf: archive extraction completed in 12.418s
DEBUG install texlive-texmf: file scan and metadata collection completed in 44.903s
DEBUG install texlive-texmf: owner conflict check completed in 1.731s
DEBUG install texlive-texmf: filesystem copy into target root completed in 318.442s
DEBUG install texlive-texmf: database update completed in 201.557s
DEBUG install texlive-texmf: total completed in 579.395s
```

This breakdown is often more useful than CPU usage alone when diagnosing slow
installs of very large small-file packages.

## Build Logs (per-stage files)

`wright build` captures build tool output (make, cmake, etc.) to per-stage files
under `<build_dir>/<name>-<version>/log/`. Every run recreates this directory
so logs are always fresh.

For the full layout, log format, and recreation rules per operation, see
[build-mechanics.md — Log Files](build-mechanics.md#log-files).

### Seeing Output in Real Time

To stream subprocess output to the terminal instead of capturing it:

```bash
wright build -v <target>
```

Verbose subprocess output is mirrored to stderr, not stdout. This keeps
`wright build --print-archives` safe to pipe into `wright install` while still
showing live build logs.

When Wright runs multiple build tasks in parallel, `-v` still keeps subprocess
output captured per task to avoid interleaving noise. For fully live output,
build a single target or narrow the build set.

## Configuration

### Change the Build Log Location

Build logs follow the build directory. Configure it in `wright.toml`:

```toml
[build]
build_dir = "/var/tmp/wright-build" # build logs end up under <build_dir>/<name>-<version>/log
```

If you want persistent logs, choose a non-temporary path.

### `log_dir` (Operation Logs)

`[general].log_dir` is reserved for system/operation logs and defaults to
`/var/log/wright` (root) or `~/.local/state/wright` (non-root). It is not wired
to build logs, which always live under `build_dir` today.
