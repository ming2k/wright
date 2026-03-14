# Usage Guide

## Tool Overview

Wright is split into three tools, each with a single responsibility:

| Tool | Role | Operates on |
|------|------|-------------|
| **`wbuild`** | Package constructor | `plan.toml` → `.wright.tar.zst` |
| **`wrepo`** | Repository manager | `.wright.tar.zst` → `wright.index.toml` |
| **`wright`** | System administrator | `.wright.tar.zst` → installed files on disk |

Data flows left to right through the pipeline:

```
 plan.toml ──► wbuild run ──► .wright.tar.zst ──► wrepo sync ──► wright install/upgrade
  (source)      (build)          (archive)          (index)         (system)
```

Each tool reads the output of the previous stage but never reaches into
another tool's domain. `wbuild` does not manage indices; `wrepo` does not
install files; `wright` does not build packages.

---

## Wbuild — Package Constructor

`wbuild` transforms `plan.toml` source descriptions into binary
`.wright.tar.zst` archives. Built archives land in `components_dir`
(`/var/lib/wright/components` by default).

### Building

```bash
wbuild run hello                    # build a single package
wbuild run @core                    # build all plans in an assembly
wbuild run @core @devel             # combine multiple assemblies
```

Plans are loaded from `plans_dir` (default: `/var/lib/wright/plans`). For
non-root setups, override `plans_dir` to a writable user-owned path. You can
also pass a path directly.

Before building, wbuild displays a **Construction Plan** showing what will be
built and why:

- `[NEW]`: The target you requested, or a missing dependency found in the hold tree.
- `[LINK-REBUILD]`: Packages that depend on your target via `link` and must be rebuilt for ABI compatibility.
- `[REV-REBUILD]`: Transitive rebuilds requested via `--rebuild-dependents`.

### Build and Install (shortcut)

The `--install` (`-i`) flag bypasses the index step — each package is
installed immediately after a successful build:

```bash
wbuild run -i curl                  # build and install in one step
wbuild run -ic @qemu                # build assembly, skip already-installed
```

This is the fastest path for single-package iteration. For batch workflows
(building many packages, upgrading later), use the full pipeline with `wrepo`.

### Staged Builds

Use `--stage` to re-run specific lifecycle stages without a full rebuild:

```bash
wbuild run hello --stage compile
wbuild run hello --stage compile --stage staging --stage fabricate
```

The build directory (`/var/tmp/wright-build/<name>-<version>/`) is preserved
between staged runs for inspection.

### Validating and Updating

```bash
wbuild check hello              # validate plan.toml syntax and deps
wbuild fetch hello              # download sources only
wbuild checksum zlib            # download sources, fill in sha256
```

### Assemblies

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

---

## Wrepo — Repository Manager

`wrepo` manages the index that sits between built archives and the system
administrator. It scans `.wright.tar.zst` files, generates a
`wright.index.toml` catalogue, and manages source configuration so
`wright` can resolve packages by name.

### Indexing

After building packages with `wbuild`, index them so `wright` can find
them by name:

```bash
wrepo sync                          # index the default components_dir
wrepo sync /path/to/custom/repo     # index a specific directory
```

**Re-run `wrepo sync` whenever you add or update packages in a repo.**

### Searching and Listing

```bash
wrepo list                          # all indexed parts
wrepo list gcc                      # all available versions of gcc
wrepo search curl                   # search by keyword (name + description)
```

Installed versions are marked with `[installed]`.

### Cleaning Up

```bash
wrepo remove gcc 14.2.0-2           # remove index entry only
wrepo remove gcc 14.2.0-2 --purge   # also delete the archive file
```

### Source Configuration

Sources tell `wright` where to look for packages. The default
`components_dir` is always searched automatically. Additional sources
are managed via `wrepo source`:

```bash
wrepo source add myrepo --path /var/lib/wright/myrepo
wrepo source add myrepo --path /var/lib/wright/myrepo --priority 300
wrepo source list
wrepo source remove myrepo
```

When the same package exists in multiple sources, the source with the
higher `priority` wins. See [Repositories](repositories.md) for details.

---

## Wright — System Administrator

`wright` manages the live system: installing, upgrading, removing, and
querying packages. It reads from the repository index (managed by `wrepo`)
and the local package database.

### Installing and Upgrading

```bash
wright install hello-1.0.0-1-x86_64.wright.tar.zst   # from a file
wright install curl                                    # by name (resolved from index)
wright install @base                                   # all packages in a kit
wright install @base @devel curl                       # mix kits and packages
wright upgrade gcc                                     # upgrade to latest available version
wright upgrade gcc --version 14.2.0                    # switch to a specific version
wright upgrade curl-8.18.0-1-x86_64.wright.tar.zst    # upgrade from a file
wright sysupgrade                                      # upgrade everything to latest
```

Wright handles dependencies, conflicts, and package replacements (renames)
automatically during installation.

**Kits** are named groups of packages (distinct from assemblies, which group
plans). Like assemblies, kits are non-dependent and combinatory — packages in
a kit are independent items bundled for convenience, not a dependency chain.
Multiple kits can be freely combined in one command, and overlapping packages
are deduplicated. Define them in `/var/lib/wright/kits/*.toml` — see
[Configuration](configuration.md#kits-package-groups) for details.

### Removing Packages

```bash
wright remove nginx                # remove a single package
wright remove --cascade nginx      # remove nginx and its orphan dependencies
wright list --orphans              # show auto-installed deps no longer needed
```

When packages are installed, wright tracks whether each was explicitly
requested (`explicit`) or pulled in automatically as a dependency
(`dependency`). `--cascade` uses this information to clean up dependencies
that are no longer needed — similar to `apt autoremove` or `pacman -Rsu`.

If you later explicitly install a package that was previously pulled in as a
dependency, `wright install` promotes it to `explicit` so it won't be removed
by cascade operations. Upgrading via `wright upgrade` or `wbuild run -icf`
preserves the existing install reason — only `wright install` expresses the
intent to "own" a package.

### Querying and Analysis

```bash
wright list --roots          # show top-level installed packages
wright search nginx          # search installed packages by keyword
wright query nginx           # show detailed info
wright deps --all            # show full dependency hierarchy
wright files nginx           # list files owned by a package
wright owner /usr/bin/nginx  # find which package owns a file
```

### Health Check

```bash
wright doctor                # diagnose database, dependencies, and file conflicts
wright verify nginx          # verify file integrity (SHA-256)
wright verify                # verify all installed packages
```

---

## How the Tools Work Together

### Standard Pipeline

The most common workflow builds a package, indexes it, then installs by name:

```bash
wbuild run gcc               # 1. build → .wright.tar.zst in components_dir
wrepo sync                   # 2. index → wright.index.toml updated
wright upgrade gcc           # 3. install → resolver finds latest from index
```

### Quick Iteration (skip the index)

For single-package development, bypass `wrepo` entirely:

```bash
wbuild run -i curl           # build and install in one step
```

### Batch Build with Deferred Install

Build many packages first, index once, then install or upgrade:

```bash
wbuild run @core             # build all core packages
wrepo sync                   # index everything at once
wright sysupgrade            # upgrade all installed packages to latest
```

### Shared Team Repository

A builder machine produces packages; developer machines consume them:

```bash
# Builder machine
wbuild run @core @devel
wrepo sync

# Developer machine (one-time setup)
wrepo source add team --path /mnt/nfs/wright-components

# Developer machine (daily use)
wright install @base
wright sysupgrade
```

### Full Rebuild with ABI Cascade

When a core library changes, rebuild it and everything that links against it:

```bash
wbuild run openssl --self --dependents   # rebuild openssl + link dependents
wrepo sync                               # re-index
wright sysupgrade                        # upgrade all affected packages
```

---

## Boundary Summary

| Concern | Handled by | Never by |
|---------|-----------|----------|
| Building from source | `wbuild` | `wright`, `wrepo` |
| Repository indexing | `wrepo` | `wright`, `wbuild` |
| Source configuration | `wrepo` | `wright`, `wbuild` |
| Searching available packages | `wrepo` | `wright` |
| Installing/removing/upgrading | `wright` | `wbuild`, `wrepo` |
| Searching installed packages | `wright` | `wrepo` |
| Querying system state | `wright` | `wbuild`, `wrepo` |
| Dependency resolution (build) | `wbuild` | `wright`, `wrepo` |
| Dependency resolution (install) | `wright` | `wbuild`, `wrepo` |
