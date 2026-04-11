# Current Design Summary

This document replaces the older historical spec. Wright is now a
source-first, local-first system with one primary CLI, distinct build/system
subcommands, and one local archive inventory.

## Core Objects

- `plan`: the source definition for one buildable unit
- `part`: a built `.wright.tar.zst` archive
- `assembly`: a named set of plans used as a build or apply target
- `system`: the installed live state tracked in `parts.db`
- `inventory`: the local catalog of built archives tracked in `inventory.db`

There is no separate indexing/publish manager and no install-time grouping
model beyond assemblies.

## Tool Boundaries

- `wright build` builds parts from plans and records successful outputs in the
  local inventory
- `wright` installs, upgrades, removes, verifies, and applies those locally
  available parts to the live system

The main workflows are:

```bash
wright build curl
wright install curl

wright apply @base

wright resolve openssl --include-targets --dependents=all --depth=0 | wright build --force --print-archives | wright install
```

## Intended Workflow

Wright is optimized for self-hosted maintenance:

- plans are the source of truth
- built archives exist mainly for rollback, recovery, and local reuse
- `wright apply` is the preferred command when you want the system to match
  current plans or assemblies
- `wright prune` cleans stale or stray archives from the local store

## Data Layout

Typical paths:

```text
/etc/wright/wright.toml
/var/lib/wright/plans/
/var/lib/wright/assemblies/
/var/lib/wright/components/
/var/lib/wright/db/parts.db
/var/lib/wright/db/inventory.db
/var/lib/wright/lock/parts.db.lock
/var/lib/wright/lock/inventory.db.lock
```

`parts.db` tracks installed system state. `inventory.db` tracks built archives
available for reuse or installation.

## Design Constraints

- build and install are separate phases
- successful builds are registered automatically in the local inventory
- install and upgrade resolution uses only the local inventory
- assemblies are the only built-in grouping abstraction
- published binary distribution is out of scope for the default architecture

For command details and current examples, use the rest of `docs/`.
