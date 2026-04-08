# Logging

This page explains where Wright writes logs, how to control verbosity, and which
settings affect log locations.

## What Gets Logged

Wright has two logging channels:

- **CLI logs**: structured logs from `wright` and `wbuild` themselves.
- **Build logs**: per-stage stdout/stderr captured for part builds.

They are configured separately.

## Log Format

CLI log lines follow a consistent format. Human-facing `INFO` messages use
natural sentence order, while some scheduler-oriented messages still carry
structured fields such as `plan=` when that improves filtering during
multi-package builds:

```
INFO cpus: 16, dockyards: 16
INFO scheduling batch 0 build: go
INFO plan=go started
INFO fetched go1.26.2.linux-amd64.tar.gz
INFO plan=go part stored in /var/lib/wright/components/go-1.26.2-1-x86_64.wright.tar.zst
INFO Installing go: 16681 files
INFO Installed go: 1.26.2-1
```

The `plan=` field still appears on scheduler and artifact-storage messages where
it makes it straightforward to filter logs for a single part during parallel
builds:

```bash
wbuild run ... 2>&1 | grep 'plan=go '
```

## CLI Verbosity (wright + wbuild)

Both `wright` and `wbuild` use the same verbosity flags:

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

`wbuild run` captures build tool output (make, cmake, etc.) to per-stage files
under `<build_dir>/<name>-<version>/log/`. Every run recreates this directory
so logs are always fresh.

For the full layout, log format, and recreation rules per operation, see
[build-mechanics.md — Log Files](build-mechanics.md#log-files).

### Seeing Output in Real Time

To stream subprocess output to the terminal instead of capturing it:

```bash
wbuild run -v <target>
```

With multiple dockyards (`-w > 1`), `-v` keeps output captured per dockyard to
avoid interleaving noise. Use `-w 1 -v` for fully live output.

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
