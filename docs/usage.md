# Usage Guide

## Overview

Wright is source-first:

- `wright build` manufactures local `.wright.tar.zst` parts from plans
- `wright build` records those parts in a local inventory database
- `wright` applies those local parts to the live system

There is no required indexing or publish stage in the default workflow.

## Build

`wright build` owns manufacturing.

### Common Commands

```bash
wright build hello
wright build @base
wright resolve @base --deps --match=outdated | wright build
wright build hello --lint
wright build zlib --checksum
```

### Dependency Scope

- `wright build` builds exactly what it receives.
- `wright resolve` expands upstream dependencies and downstream rebuilds before the build starts.
- `wright resolve --deps --match=outdated` is the usual maintenance mode when you want outdated upstream dependencies rebuilt before the target.

### Part Inventory

Successful builds are written to `parts_dir` and registered in
`inventory_db_path`.

To clean old or stray parts:

```bash
wright prune --untracked
wright prune --latest --apply
```

`--latest` keeps the newest tracked part per part name while preserving any
currently installed versions. `--untracked` removes part files that exist on
disk but are not registered in the inventory DB.

## Wright

`wright` owns live-system mutation.

### Install and Upgrade

```bash
wright install ./hello-1.0.0-1-x86_64.wright.tar.zst
wright install hello
wright upgrade hello
wright upgrade hello --version=1.0.0
wright sysupgrade
```

Part names are resolved from the local part inventory. 

**File Diversion**: If an incoming part contains files that conflict with paths owned by another installed part, Wright automatically diverts the original files by renaming them with a `.wright-diverted` extension rather than aborting the transaction. When the new part is later removed, the diverted files are restored to their original paths.

### Apply Assemblies

`wright apply` is the preferred plan-driven combo command when plans are the
source of truth. Use it as the natural default for first install,
incremental upgrade, and dependency handling from plans:

```bash
wright apply @base
wright apply @base @devel
wright apply ./plans/bash
wright apply @base --dry-run
```

`wright apply`:

1. resolves the requested plans or assemblies
2. automatically adds missing or outdated upstream dependency plans to the build graph
3. computes dependency waves for the required build graph
4. for each wave, builds what is needed there
5. installs or upgrades that wave before continuing, so later waves see the updated system state

If the requested targets already match the current plan state under the
selected policy, `wright apply` becomes a no-op instead of failing.

Useful knobs:

- `--dry-run` previews what would be built and installed without mutating the system
- `--force`, `-f` forces a clean rebuild and re-installation even if matching parts already exist in the inventory; source downloads are still reused from cache
- `--match` overrides the default `outdated` policy when you want a different install-state filter

For the design rationale behind this command's defaults and wave model, see
[Apply Design](apply-design.md).

## Remove and Inspect

```bash
wright remove nginx
wright remove --cascade nginx
wright list --orphans
wright deps nginx --reverse
wright query nginx
wright files nginx
wright verify
wright doctor
```

## Typical Workflows

### First Build

```bash
wright build hello
wright install hello
```

### Source-First Maintenance

```bash
wright apply @base
wright sysupgrade
wright prune --untracked --latest --apply
```

### Explicit Rebuild Scope

```bash
wright resolve openssl --rdeps=all --depth=0 | wright build --force
wright upgrade openssl
```

## Install Origins

Wright tracks install origins:

- `manual`
- `dependency`

`wright remove --cascade` only cleans dependency-origin parts that are no longer
required.
