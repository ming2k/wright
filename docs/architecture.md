# Architecture

Wright is a single CLI binary backed by one core library.

## Roles

| CLI surface | Role |
|-------------|------|
| `wright build`, `wright resolve`, `wright prune` | manufacture parts from plans and maintain the local archive inventory |
| `wright install`, `wright upgrade`, `wright apply`, other system subcommands | apply locally available parts to the live system |

## Data Flow

```text
plan.toml -> wright build -> .wright.tar.zst -> archives.db -> wright install/upgrade/apply
```

## Entry Points and Layers

```text
src/
├── bin/
├── archive/   # archive pruning and resolution logic
├── builder/   # build orchestration and lifecycle execution
├── cli/       # clap schemas grouped by subcommand
├── commands/  # command handlers grouped by subcommand
├── config.rs  # global config and assembly definitions
├── database/  # unified database layer (installed system + archive catalogue)
├── isolation/  # sandbox isolation
├── part/      # archive format, versions, FHS validation
├── plan/      # plan parsing and validation
├── query/     # system analysis
├── transaction/ # install / upgrade / remove / verify
└── util/      # helpers
```

The execution path is intentionally thin at the top:

```text
src/bin/wright.rs -> src/cli/* -> src/commands/* -> library modules
```

- `src/bin/wright.rs` parses args, initializes logging, loads config, and dispatches.
- `src/cli/` owns clap-facing argument and help-text definitions only.
- `src/commands/` turns parsed args into calls into `builder`, `archive`, `transaction`, and `query`.

## Responsibilities

### `wright build` / `wright resolve` / `wright prune`

- resolve plans and assemblies
- expand dependency and rebuild scope
- execute sandboxed stages
- create `.wright.tar.zst` archives
- register build outputs in `archives.db`
- prune stale archives

### `wright`

- resolve local part names from `archives.db`
- install and upgrade archives transactionally
- remove parts and cascade orphan cleanup
- verify and inspect the live system
- run `apply` as the high-level orchestrator:
 resolve targets, execute build waves, and install each wave before advancing

## Shared State

Detailed database schemas and their roles are documented in [Database Design](database.md).

| Artifact | Written by | Read by |
|----------|-----------|---------|
| `plan.toml` | user | `wright build`, `wright resolve`, `wright apply` |
| `.wright.tar.zst` | `wright build` | `wright` |
| `installed.db` | `wright` | `wright`, `wright resolve` |
| `archives.db` | `wright build` | `wright build`, `wright`, `wright apply` |

