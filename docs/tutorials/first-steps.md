# First Steps with Wright

This tutorial walks you through the core Wright workflows after you have completed the [Getting Started](../tutorials/getting-started.md) guide.

## Overview

Wright is source-first:

- `wright build` manufactures staging and output directories from plans.
- `wright package` slices staging into output directories and creates archives.
- `wright merge` applies packaged plan outputs to the live system.

## Build Your First Part

```bash
wright build hello
```

This builds the `hello` plan into a staging directory.

## Package the Build

```bash
wright package hello
```

This re-slices `staging/` into `outputs/` according to the plan's `[[output]]` rules (if any) and creates `.wright.tar.zst` archives. Use `--force` to re-slice even when `outputs/` already exists.

## Resolve and Build

`wright build` builds exactly what it receives. To automatically add missing dependencies:

```bash
wright resolve hello --deps --match=outdated | wright build
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

`wright install` is the preferred plan-driven combo command. It resolves targets, adds missing or outdated dependencies, builds each wave, and installs or upgrades each wave before continuing:

```bash
wright install hello
wright install zlib openssl
wright install ./plans/bash
wright install hello --dry-run
```

## Install and Upgrade

```bash
wright merge hello
wright upgrade hello
wright sysupgrade
```

## Remove and Inspect

```bash
wright remove nginx
wright remove --cascade nginx
wright list --orphans
wright resolve nginx --tree --rdeps
wright query nginx
wright files nginx
wright verify
wright doctor
```

## Clean Up Old Archives

```bash
wright prune --latest --apply
```

## Typical Workflows

### First Build

```bash
wright build hello
wright package hello
wright merge hello
```

### Source-First Maintenance

```bash
wright install hello openssl
wright sysupgrade
wright prune --latest --apply
```

### Explicit Rebuild Scope

```bash
wright resolve openssl --rdeps=all --depth=0 | wright build --force
wright resolve openssl --rdeps=all --depth=0 | wright package
wright upgrade openssl
```

## Next Steps

- Learn how to [write a plan](../how-to/write-a-plan.md)
- Browse the [how-to guides](../how-to/index.md)
- Read the [explanations](../explanation/index.md)
