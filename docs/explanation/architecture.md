# Architecture

Wright is a single CLI binary backed by one core library.

## Roles

| CLI surface | Role |
|-------------|------|
| `wright build`, `wright package`, `wright resolve`, `wright prune` | manufacture parts from plans and maintain archives in `parts_dir` |
| `wright install`, `wright upgrade`, `wright apply`, other system subcommands | apply locally available parts to the live system |

## Data Flow

```text
plan.toml -> wright build -> staging/ -> wright package -> .wright.tar.zst -> wright install/upgrade/apply
```

## Responsibilities

### `wright build` / `wright resolve` / `wright prune`

- resolve plans
- expand dependency and rebuild scope
- execute sandboxed stages
- create `.wright.tar.zst` archives
- write archives to `parts_dir`
- prune stale archives

### `wright`

- resolve local part names by scanning `parts_dir` and reading `.PARTINFO`
- install and upgrade archives transactionally
- remove parts and cascade orphan cleanup
- verify and inspect the live system
- run `apply` as the high-level orchestrator:
  resolve targets, execute build waves, and install each wave before advancing

## Shared State

Detailed database schemas and their roles are documented in [Database Design](../reference/database-design.md).

| Artifact | Written by | Read by |
|----------|-----------|---------|
| `plan.toml` | user | `wright build`, `wright resolve`, `wright apply` |
| `staging/` | `wright build` | `wright package`, user inspection |
| `.wright.tar.zst` | `wright package`, `wright build --package`, `wright apply` | `wright install`, `wright upgrade`, `wright sysupgrade`, `wright apply` |
| `wright.db` | `wright` | `wright`, `wright resolve`, `wright build`, `wright apply` |

For module-level code organization, see [Module Layout](../dev/module-layout.md).
