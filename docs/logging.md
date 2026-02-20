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

`wbuild run` captures build tool output (e.g. `make`, `cmake`) to files instead
of echoing it to the terminal. Logs are written under the build directory:

```
<build_dir>/<name>-<version>/log/
```

The default `build_dir` is `/tmp/wright-build`, so a typical log path looks like:

```
/tmp/wright-build/zlib-1.3.1/log/compile.log
```

Each lifecycle stage writes a log named `<stage>.log` and includes:

- stage name
- exit code
- duration (seconds)
- stdout and stderr

Split packages also emit `package-<split>.log` logs.

### When Logs Are Recreated

Every build run recreates `log/` for a clean result, so logs from earlier runs
with the same `<name>-<version>` are overwritten. `--only` also recreates `log/`
to keep the output fresh.

### Seeing Output in Real Time

If you want live output in the terminal, use `-v`:

```bash
wbuild run -v <target>
```

Note: when building with multiple workers (`-w > 1`), `-v` keeps subprocess output captured
to avoid interleaving noise.

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
