# Logging

This page explains where Wright writes logs, how to control verbosity, and which
settings affect log locations.

## What Gets Logged

Wright has two logging channels:

- **CLI logs**: structured logs from `wright` and `wbuild` themselves.
- **Build logs**: per-stage stdout/stderr captured for part builds.

They are configured separately.

## Log Format

CLI log lines follow a consistent format: all messages are lowercase, and logs
scoped to a specific plan carry a `plan=` structured field so the plan name is
clearly separated from the message text:

```
INFO cpus: 16, dockyards: 16
INFO scheduling batch 0 build: go
INFO plan=go started
INFO fetched go1.26.2.linux-amd64.tar.gz
INFO plan=go part stored in /var/lib/wright/components/go-1.26.2-1-x86_64.wright.tar.zst
INFO plan=go installing: 16681 files
INFO plan=go installed 1.26.2-1
```

The `plan=` field makes it straightforward to filter logs for a single part when
building multiple packages in parallel:

```bash
wbuild run ... 2>&1 | grep 'plan=go '
```

## CLI Verbosity (wright + wbuild)

Both `wright` and `wbuild` use the same verbosity flags:

- `-v` enables debug logs.
- `-vv` enables trace logs.
- `--quiet` shows warnings and errors only.

Default level is `info`. Logs are printed to stderr by the CLI.

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
