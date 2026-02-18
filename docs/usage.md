# Usage Guide

Wright is split into two tools: `wright` (system management) and `wbuild` (package construction).

## Wright (System Administrator)

Use `wright` to manage the live system.

### Installing and Upgrading

```bash
wright install hello-1.0.0-1-x86_64.wright.tar.zst
wright upgrade curl-8.18.0-1-x86_64.wright.tar.zst
```

Wright handles dependencies, conflicts, and package replacements (renames) automatically during installation.

### Querying and Analysis

```bash
wright list --roots          # Show top-level installed packages
wright query nginx           # Show detailed info
wright deps openssl --tree   # Show full dependency hierarchy
```

### Health Check

```bash
wright doctor                # Diagnose database, dependencies, and file conflicts
```

---

## Wbuild (Package Constructor)

Use `wbuild` to transform `plan.toml` files into binary packages.

### Building

```bash
wbuild run hello
```

Plans are loaded from `plans_dir` (default: `/var/lib/wright/plans`). You can also pass a path directly.

Before building, Wright displays a **Construction Plan** showing what will be built and why:
- `[NEW]`: The target you requested, or a missing dependency that Wright found in the hold tree.
- `[LINK-REBUILD]`: Packages that depend on your target via `link` and must be rebuilt for ABI compatibility.
- `[REV-REBUILD]`: Transitive rebuilds requested via `--rebuild-dependents`.

### One-Stop Build and Install

The most efficient way to manage a package from source is using the `--install` (or `-i`) flag:

```bash
wbuild run -i curl
```

This command does the following:
1.  Analyzes `curl`'s dependencies.
2.  If any `build` or `link` dependencies are not installed, it searches for them in the hold tree.
3.  Recursively adds all missing plans to the construction plan.
4.  Starts parallel compilation.
5.  Immediately installs each package after it finishes building.

### Staged Builds

Use `--stage` to stop the pipeline after a specific stage, and `--only` to run a single stage in isolation:

```bash
wbuild run --stage configure hello      # stop after configure
wbuild run --only compile hello         # run only the compile stage
```

The build directory (`/tmp/wright-build/<name>-<version>/`) is preserved after a staged build for inspection.

### Validating and Updating

```bash
wbuild check hello              # validate syntax only
wbuild update zlib              # download sources, fill in sha256
```

### Assembly Builds

Group related plans for batch building:

```bash
wbuild run @core
```