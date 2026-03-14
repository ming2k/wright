# Usage Guide

Wright is split into two tools: `wright` (system management) and `wbuild` (package construction).

## Typical Workflow

```
 plan.toml ──► wbuild run ──► .wright.tar.zst ──► wright repo sync ──► wright install/upgrade
  (source)      (build)          (parts)            (index repo)         (manage system)
```

1. **Write a plan** — define how to build a package in `plan.toml`
2. **Build** — `wbuild run mypackage` compiles and creates `.wright.tar.zst` archives
3. **Index** — `wright repo sync <dir>` scans the built parts and generates `wright.index.toml`
4. **Install or upgrade** — `wright install mypackage` or `wright upgrade mypackage` resolves the name from the index

For quick iteration, `wbuild run -i mypackage` builds and installs in one step (skipping the index cycle).

## Wright (System Administrator)

Use `wright` to manage the live system.

### Repositories

See [Repositories](repositories.md) for the full guide on creating local repos,
managing sources, indexing, and syncing.

### Installing and Upgrading

```bash
wright install hello-1.0.0-1-x86_64.wright.tar.zst   # from a file
wright install curl                                    # by name (resolved from sources)
wright install @base                                   # all packages in a kit
wright install @base @devel curl                       # mix kits and packages
wright upgrade gcc                                     # upgrade to latest available version
wright upgrade gcc --version 14.2.0                    # switch to a specific version
wright upgrade curl-8.18.0-1-x86_64.wright.tar.zst    # upgrade from a file
wright sysupgrade                                      # upgrade everything to latest
```

Wright handles dependencies, conflicts, and package replacements (renames) automatically during installation.

**Kits** are named groups of packages (distinct from assemblies, which group plans). Like assemblies, kits are non-dependent and combinatory — packages in a kit are independent items bundled for convenience, not a dependency chain. Multiple kits can be freely combined in one command, and overlapping packages are deduplicated. Define them in `/var/lib/wright/kits/*.toml` — see [Configuration](configuration.md#kits-package-groups) for details.

### Removing Packages

```bash
wright remove nginx                # Remove a single package
wright remove --cascade nginx      # Remove nginx and its orphan dependencies
wright list --orphans              # Show auto-installed deps no longer needed
```

When packages are installed, wright tracks whether each was explicitly requested (`explicit`) or pulled in automatically as a dependency (`dependency`). `--cascade` uses this information to clean up dependencies that are no longer needed — similar to `apt autoremove` or `pacman -Rsu`.

If you later explicitly install a package that was previously pulled in as a dependency, `wright install` promotes it to `explicit` so it won't be removed by cascade operations. Upgrading via `wright upgrade` or `wbuild run -icf` preserves the existing install reason — only `wright install` expresses the intent to "own" a package.

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

Plans are loaded from `plans_dir` (default: `/var/lib/wright/plans`). For
non-root setups, override `plans_dir` to a writable user-owned path. You can
also pass a path directly.

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
wbuild run hello --stage compile --stage staging --stage fabricate   # re-run compile through final output
```

To skip the `check` stage — for example when tests are slow or broken upstream — run everything except `check`:

```bash
wbuild run hello --stage prepare --stage configure --stage compile --stage staging --stage fabricate
```

The build directory (`/var/tmp/wright-build/<name>-<version>/` by default) is preserved between staged runs for inspection.

### Validating and Updating

```bash
wbuild check hello              # validate syntax only
wbuild checksum zlib            # download sources, fill in sha256
```

### Repository Management

After building parts, generate an index so `wright` can resolve parts by name:

```bash
wright repo sync /var/lib/wright/components   # index a directory
wright repo list gcc                          # see all available versions
wright repo remove gcc 14.2.0-2 --purge      # clean up old versions
```

This creates a `wright.index.toml` in the target directory. With an index present, the resolver uses fast lookups instead of scanning every archive. `wbuild index` also works as an alternative.

### Assembly Builds

Assemblies are named collections of plans that can be built together.
Membership is purely combinatory — items in an assembly are independent
units bundled for convenience, not a dependency chain. Actual build
ordering comes from the dependency graph in each plan.

```bash
wbuild run @core                # build all plans in the "core" assembly
wbuild run @core @devel         # combine multiple assemblies
wbuild run -ic @qemu            # build and install, skipping already-installed parts
```

When used with `--install` (`-i`), parts already installed on the system
are automatically skipped. Use `--force` (`-f`) to rebuild them anyway.
