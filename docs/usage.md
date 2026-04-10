# Usage Guide

## Overview

Wright is source-first:

- `wbuild` manufactures local `.wright.tar.zst` archives from plans
- `wbuild` records those archives in a local inventory database
- `wright` applies those local archives to the live system

There is no required indexing or publish stage in the default workflow.

## Wbuild

`wbuild` owns manufacturing.

### Common Commands

```bash
wbuild run hello
wbuild run @base
wbuild resolve @base --self --deps=sync | wbuild run
wbuild check hello
wbuild checksum zlib
```

### Dependency Scope

- `wbuild run` builds exactly what it receives.
- `wbuild resolve` expands upstream dependencies and downstream rebuilds before the build starts.
- `--deps=sync` is the usual maintenance mode: it rebuilds dependencies whose installed versions no longer match the current plans.

### Archive Inventory

Successful builds are written to `components_dir` and registered in
`inventory_db_path`.

To clean old or stray archives:

```bash
wbuild prune --untracked
wbuild prune --latest --apply
```

`--latest` keeps the newest tracked archive per part name while preserving any
currently installed versions. `--untracked` removes archive files that exist on
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

Part names are resolved from the local archive inventory.

### Apply Assemblies

`wright apply` is the preferred maintenance command when plans are the source of
truth:

```bash
wright apply @base
wright apply @base @devel
wright apply ./plans/bash
```

`wright apply`:

1. resolves the requested plans or assemblies
2. computes dependency waves for the required build graph
3. for each wave, builds any missing or outdated archives needed there
4. installs that wave before continuing, so later waves see the updated system state

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
wbuild run hello
wright install hello
```

### Source-First Maintenance

```bash
wright apply @base
wright sysupgrade
wbuild prune --untracked --latest --apply
```

### Explicit Rebuild Scope

```bash
wbuild resolve openssl --self --dependents=all --depth=0 | wbuild run --force
wright upgrade openssl
```

## Install Origins

Wright tracks install origins:

- `manual`
- `dependency`

`wright remove --cascade` only cleans dependency-origin parts that are no longer
required.
