# Logging

This page explains where Wright writes logs, how to control verbosity, and which
settings affect log locations.

## What Gets Logged

Wright has two logging channels:

- **CLI logs**: structured logs from `wright` and `wbuild` themselves.
- **Build logs**: per-stage stdout/stderr captured for package builds.

They are configured separately.

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
