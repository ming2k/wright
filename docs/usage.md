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
wright deps --all            # Show full dependency hierarchy
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

Use `--stage` to run only specific lifecycle stages. Repeat it to run multiple stages. Requires a previous full build (fetch/verify/extract are skipped):

```bash
wbuild run hello --stage compile         # re-run only compile
wbuild run hello --stage compile --stage package   # re-run compile then package
```

To skip the `check` stage — for example when tests are slow or broken upstream — run everything except `check`:

```bash
wbuild run hello --stage prepare --stage configure --stage compile --stage package --stage post_package
```

The build directory (`/tmp/wright-build/<name>-<version>/`) is preserved between staged runs for inspection.

### Validating and Updating

```bash
wbuild check hello              # validate syntax only
wbuild checksum zlib            # download sources, fill in sha256
```

### Assembly Builds

Group related plans for batch building:

```bash
wbuild run @core
```