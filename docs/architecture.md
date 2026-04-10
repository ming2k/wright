# Architecture

Wright is split into two binaries that share one core library.

## Roles

| Binary | Role |
|--------|------|
| `wbuild` | manufacture parts from plans and maintain the local archive inventory |
| `wright` | apply locally available parts to the live system |

## Data Flow

```text
plan.toml -> wbuild run -> .wright.tar.zst -> inventory.db -> wright install/upgrade/apply
```

## Core Modules

```text
src/
├── bin/
│   ├── wbuild.rs
│   └── wright.rs
├── builder/      # build orchestration and lifecycle execution
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

### `wbuild`

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
| `plan.toml` | user | `wbuild`, `wright apply` |
| `.wright.tar.zst` | `wbuild` | `wright` |
| `parts.db` | `wright` | `wright`, `wbuild resolve` |
| `inventory.db` | `wbuild` | `wbuild`, `wright` |
