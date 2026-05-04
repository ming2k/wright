# Design Specification

This document replaces the older historical spec. Wright is now a
source-first, local-first system with one primary CLI, distinct build/system
subcommands, and a single state database.

## Core Objects

- `plan`: the source definition for one buildable unit
- `part`: a built `.wright.tar.zst` archive
- `system`: the installed live state tracked in `wright.db`

## Tool Boundaries

- `wright build` builds parts from plans and creates `.wright.tar.zst` archives
- `wright package` slices staging directories into output directories (`outputs/`) and packages them into `.wright.tar.zst` archives
- `wright install` installs locally available archives to the live system
- `wright apply` resolves, builds, and installs plans in dependency waves

The main workflows are:

```bash
wright build curl
wright package curl
wright install ./curl-8.0-1-x86_64.wright.tar.zst

# Or the all-in-one apply workflow:
wright apply curl
```

## File Model

A `plan.toml` lives in its own directory under `plans_dir`. Each plan is self-contained:

```
plans/curl/plan.toml
```

## Output Model

Each build produces one or more `.wright.tar.zst` archives under `parts_dir`. A plan
can have multiple outputs (e.g. `gcc` and `gcc-libs`) defined by `[[output]]` tables.

## State Model

`wright.db` is the single source of truth for:

- installed parts and their files
- dependency relationships
- transaction history
- build/apply resume sessions

## CLI Architecture

```
wright build   →  build plans
wright package →  slice staging into outputs and package
wright apply   →  resolve + build + install
wright install →  install archives
wright upgrade →  upgrade installed parts
wright remove  →  remove installed parts
wright list    →  list installed parts
wright resolve →  inspect dependency graph
wright lint    →  validate plan files
wright prune   →  clean old archives
```

## Isolation Model

Build stages run in optional sandboxed environments. The default isolation level
is `strict`. Each stage can override this via its `isolation` field.

## Concurrency Model

Builds run in dependency-ordered waves. Plans in the same wave build in parallel.
The scheduler divides available CPUs across active isolations.
