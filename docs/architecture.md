# Architecture

Wright is a single CLI binary backed by one core library.

## Roles

| CLI surface | Role |
|-------------|------|
| `wright build`, `wright resolve`, `wright prune` | manufacture parts from plans and maintain the local archive inventory |
| `wright install`, `wright upgrade`, `wright apply`, other system subcommands | apply locally available parts to the live system |

## Data Flow

```text
plan.toml -> wright build -> .wright.tar.zst -> inventory.db -> wright install/upgrade/apply
```

## Core Modules

```text
src/
├── bin/
│   └── wright.rs
├── builder/      # build orchestration and lifecycle execution
├── cli/          # clap definitions for system/build subcommands
├── config.rs     # global config and assembly definitions
├── database/     # installed-system DB
├── dockyard/     # sandbox isolation
├── inventory/    # local archive inventory DB + resolver
├── part/         # archive format, versions, FHS validation
├── plan/         # plan parsing and validation
├── query/        # system analysis
├── transaction/  # install / upgrade / remove / verify
└── util/         # helpers
```

## Responsibilities

### `wright build` / `wright resolve` / `wright prune`

- resolve plans and assemblies
- expand dependency and rebuild scope
- execute sandboxed stages
- create `.wright.tar.zst` archives
- register build outputs in `inventory.db`
- prune stale archives

### `wright`

- resolve local part names from `inventory.db`
- install and upgrade archives transactionally
- remove parts and cascade orphan cleanup
- verify and inspect the live system
- run `apply` as the high-level orchestrator:
  resolve targets, execute build waves, and install each wave before advancing

## Shared State

| Artifact | Written by | Read by |
|----------|-----------|---------|
| `plan.toml` | user | `wright build`, `wright resolve`, `wright apply` |
| `.wright.tar.zst` | `wright build` | `wright` |
| `parts.db` | `wright` | `wright`, `wright resolve` |
| `inventory.db` | `wright build` | `wright build`, `wright`, `wright apply` |
