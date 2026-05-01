# First Steps with Wright

This tutorial walks you through the core Wright workflows after you have completed the [Getting Started](../tutorials/getting-started.md) guide.

## Overview

Wright is source-first:

- `wright build` manufactures local `.wright.tar.zst` parts from plans.
- `wright build` records those parts in a local inventory database.
- `wright` applies those local parts to the live system.

There is no required indexing or publish stage in the default workflow.

## Build Your First Part

```bash
wright build hello
```

This builds the `hello` plan into a `.wright.tar.zst` archive and registers it in the local inventory.

## Build an Assembly

Assemblies are named groups of plans:

```bash
wright build @base
```

## Resolve and Build

`wright build` builds exactly what it receives. To automatically add missing dependencies:

```bash
wright resolve @base --deps --match=outdated | wright build
```

## Lint a Plan

Validate a plan's syntax and dependency graph before building:

```bash
wright lint hello
```

## Update Checksums

Automatically compute and update SHA-256 checksums in `plan.toml`:

```bash
wright build zlib --checksum
```

## Apply Plans to the Live System

`wright apply` is the preferred plan-driven combo command. It resolves targets, adds missing or outdated dependencies, builds each wave, and installs or upgrades each wave before continuing:

```bash
wright apply @base
wright apply @base @devel
wright apply ./plans/bash
wright apply @base --dry-run
```

## Install and Upgrade

```bash
wright install ./hello-1.0.0-1-x86_64.wright.tar.zst
wright install hello
wright upgrade hello
wright sysupgrade
```

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

## Clean Up the Inventory

```bash
wright prune --untracked
wright prune --latest --apply
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

## Next Steps

- Learn how to [write plans](../reference/writing-plans.md)
- Learn how to [write assemblies](../reference/writing-assemblies.md)
- Browse the [how-to guides](../how-to/index.md)
- Read the [explanations](../explanation/index.md)
