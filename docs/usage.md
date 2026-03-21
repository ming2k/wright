# Usage Guide

## Tool Overview

Before the command details, keep the project's core metaphor in mind:

- the live machine is the **system**
- a `plan.toml` is a **plan** for manufacturing one **part**
- a `.wright.tar.zst` archive is the finished **part**
- `wrepo` manages the repository of finished parts
- `wright` installs and replaces parts on the live system

This vocabulary is intentional. Wright avoids collapsing the build definition, the built artifact, the repository entry, and the installed state into one vague word. See [terminology.md](terminology.md) for the canonical glossary.

Wright is split into three tools, each with a single responsibility:

| Tool | Role | Operates on |
|------|------|-------------|
| **`wbuild`** | Part constructor | `plan.toml` → `.wright.tar.zst` |
| **`wrepo`** | Repository manager | `.wright.tar.zst` → `wright.index.toml` |
| **`wright`** | System administrator | `.wright.tar.zst` → installed files on disk |

Data flows left to right through the pipeline:

```
 plan.toml ──► wbuild run ──► .wright.tar.zst ──► wrepo sync ──► wright install/upgrade
  (source)      (build)          (archive)          (index)         (system)
```

Each tool reads the output of the previous stage but never reaches into
another tool's domain. `wbuild` does not manage indices; `wrepo` does not
install files; `wright` does not build parts.

---

## Wbuild — Part Constructor

`wbuild` transforms `plan.toml` source descriptions into binary
`.wright.tar.zst` archives. Built archives land in `components_dir`
(`/var/lib/wright/components` by default).

### Building

```bash
wbuild run hello                    # build a single part
wbuild run @core                    # build all plans in an assembly
wbuild run @core @devel             # combine multiple assemblies
```

Plans are loaded from `plans_dir` (default: `/var/lib/wright/plans`). For
non-root setups, override `plans_dir` to a writable user-owned path. You can
also pass a path directly.

Before building, wbuild logs a scheduling plan showing what will be built
in dependency order. Each entry includes an action label and its depth in
the dependency graph:

- `build`: The target you requested, or a dependency that was added to complete the plan.
- `relink`: Parts that depend on your target via `link` and must be rebuilt for ABI compatibility.
- `rebuild`: Transitive rebuilds (from `wbuild resolve --dependents=all`).

### Build and Install (shortcut)

The `--install` (`-i`) flag bypasses the index step — each part is
installed immediately after a successful build:

```bash
wbuild run -i curl                                       # build and install in one step
wbuild resolve @qemu --self --deps=sync | wbuild run -i  # build assembly and sync deps
```

This is the fastest path for single-part iteration. For batch workflows
(building many parts, upgrading later), use the full pipeline with `wrepo`.

### Staged Builds

Use `--stage` to re-run specific lifecycle stages without a full rebuild:

```bash
wbuild run hello --stage=compile
wbuild run hello --stage=compile --stage=staging --stage=fabricate
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
wbuild run -i @qemu             # build and install the requested plans
wbuild run -i --deps=sync @qemu # also sync missing/outdated upstream deps
```

By default, `wbuild run` builds only the listed plans. Use `--deps` to expand
upstream dependencies explicitly: bare `--deps` means `missing`,
`--deps=sync` adds installed dependencies whose epoch/version/release differs
from the current `plan.toml`, and `--deps=all` rebuilds the full upstream chain.

---

## Wrepo — Repository Manager

`wrepo` manages the index that sits between built archives and the system
administrator. It scans `.wright.tar.zst` files, generates a
`wright.index.toml` catalogue, and manages source configuration so
`wright` can resolve parts by name.

### Indexing

After building parts with `wbuild`, index them so `wright` can find
them by name:

```bash
wrepo sync                          # index the default components_dir
wrepo sync ./components             # index a specific directory
```

**Re-run `wrepo sync` whenever you add or update parts in a repo.**

### Searching and Listing

```bash
wrepo list                          # all indexed parts
wrepo list zlib                     # all available versions of zlib
wrepo search zlib                   # search by keyword (name + description)
wrepo search ssl                    # search by keyword (name + description)
```

Installed versions are marked with `[installed]`.

### Cleaning Up

```bash
wrepo remove zlib 1.3.1             # remove index entry only
wrepo remove zlib 1.3.1-2 --purge   # also delete the archive file
```

### Source Configuration

Sources tell `wright` where to look for parts. The default
`components_dir` is always searched automatically. Additional sources
are managed via `wrepo source`:

```bash
wrepo source add local --path=/srv/wright/repo
wrepo source add cache --path=./repo --priority=200
wrepo source list
wrepo source remove local
```

When the same part exists in multiple sources, the source with the
higher `priority` wins. See [Repositories](repositories.md) for details.

---

## Wright — System Administrator

`wright` manages the live system: installing, upgrading, removing, and
querying parts. It reads from the repository index (managed by `wrepo`)
and the local part database.

### Installing and Upgrading

```bash
wright install hello-1.0.0-1-x86_64.wright.tar.zst   # from a file
wright install curl                                    # by name (resolved from index)
wright install @base                                   # all parts in a kit
wright install @base @devel curl                       # mix kits and named parts
wright upgrade gcc                                     # upgrade to latest available version
wright upgrade gcc --version=14.2.0                    # switch to a specific version
wright upgrade curl-8.18.0-1-x86_64.wright.tar.zst    # upgrade from a file
wright sysupgrade                                      # upgrade everything to latest
```

Wright handles dependencies, conflicts, and part replacements (renames)
automatically during installation.

**Kits** are named groups of parts (distinct from assemblies, which group
plans). Like assemblies, kits are non-dependent and combinatory — parts in
a kit are independent items bundled for convenience, not a dependency chain.
Multiple kits can be freely combined in one command, and overlapping parts
are deduplicated. Define them in `/var/lib/wright/kits/*.toml` — see
[Configuration](configuration.md#kits-part-groups) for details.

### Removing Parts

```bash
wright remove nginx                # remove a single part
wright remove --cascade nginx      # remove nginx and its orphan dependencies
wright list --orphans              # show auto-installed deps no longer needed
```

When parts are installed, wright tracks their origin: `manual` (user ran
`wright install`), `build` (installed via `wbuild run -i`), or `dependency`
(auto-resolved). `--cascade` uses this information to clean up `dependency`-origin
parts that are no longer needed — similar to `apt autoremove` or `pacman -Rsu`.

Origins follow a promotion hierarchy: `dependency → build → manual`. If you
later explicitly install a part that was previously pulled in as a dependency,
`wright install` promotes it to `manual` so it won't be removed by cascade
operations. Upgrading via `wright upgrade` or `wbuild run -icf` preserves the
existing origin — only `wright install` expresses the intent to "own" a part.

### Querying and Analysis

```bash
wright list --roots          # show top-level installed parts
wright search nginx          # search installed parts by keyword
wright query nginx           # show detailed info
wright deps --all            # show full dependency hierarchy
wright deps nginx --reverse  # show what depends on nginx
wright files nginx           # list files owned by a part
wright owner /usr/bin/nginx  # find which part owns a file
```

### Health Check

```bash
wright doctor                # diagnose database, dependencies, and file conflicts
wright verify nginx          # verify file integrity (SHA-256)
wright verify                # verify all installed parts
wright history nginx         # show transaction history for one part
```

---

## How the Tools Work Together

### Standard Pipeline

The most common workflow builds a part, indexes it, then installs by name:

```bash
wbuild run gcc               # 1. build → .wright.tar.zst in components_dir
wrepo sync                   # 2. index → wright.index.toml updated
wright upgrade gcc           # 3. install → resolver finds latest from index
```

### Quick Iteration (skip the index)

For single-part development, bypass `wrepo` entirely:

```bash
wbuild run -i curl           # build and install in one step
```

### Batch Build with Deferred Install

Build many parts first, index once, then install or upgrade:

```bash
wbuild run @core             # build all core parts
wrepo sync                   # index everything at once
wright sysupgrade            # upgrade all installed parts to latest
```

### Shared Team Repository

A builder machine produces parts; developer machines consume them:

```bash
# Builder machine
wbuild run @core @devel
wrepo sync

# Developer machine (one-time setup)
wrepo source add team --path=/mnt/nfs/wright-components

# Developer machine (daily use)
wright install @base
wright sysupgrade
```

### Full Rebuild with ABI Cascade

When a core library changes, rebuild it and everything that links against it:

```bash
wbuild resolve openssl --self --dependents | wbuild run --force -i  # rebuild + install
```

If the build fails partway through, resume without re-building completed parts:

```bash
wbuild resolve openssl --self --dependents | wbuild run --resume -i  # skip already-installed parts
```

The `--resume` flag tracks build progress in a session. On failure, the session hash is printed — you can pass it explicitly (`--resume <hash>`) or let it auto-detect from the same build set.

---

## Boundary Summary

| Concern | Handled by | Never by |
|---------|-----------|----------|
| Building from source | `wbuild` | `wright`, `wrepo` |
| Repository indexing | `wrepo` | `wright`, `wbuild` |
| Source configuration | `wrepo` | `wright`, `wbuild` |
| Searching available parts | `wrepo` | `wright` |
| Installing/removing/upgrading | `wright` | `wbuild`, `wrepo` |
| Searching installed parts | `wright` | `wrepo` |
| Querying system state | `wright` | `wbuild`, `wrepo` |
| Dependency resolution (build) | `wbuild` | `wright`, `wrepo` |
| Dependency resolution (install) | `wright` | `wbuild`, `wrepo` |
