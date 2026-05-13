# Architecture

Wright is a single CLI binary backed by one core library.

## Roles

| CLI surface | Role |
|-------------|------|
| `wright build`, `wright prune` | forge plan outputs and maintain archives in `parts_dir` |
| `wright merge`, `wright upgrade`, `wright install`, `wright launch`, other system subcommands | apply locally available parts to a target root (the live system or a fresh one) |

## Data Flow

```mermaid
flowchart LR
    Plan["plan.toml"] --> Build["wright build"]
    Build --> Staging["staging/"]
    Staging --> Package["wright package"]
    Package --> Archive[".wright.tar.zst"]
    Archive --> System["wright install / upgrade / merge"]
```

`wright install` and `wright launch` are source-first convergence operations. They
resolve requested plans, forge each dependency-safe wave, seal the resulting
outputs, and deploy each completed wave before continuing.

## Internal Layers

```text
bin -> cli -> commands -> operations -> resolve / forge / seal / deploy
```

- `cli` defines command-line schemas.
- `commands` maps CLI args into operation requests and acquires command locks.
- `operations` owns command use cases such as install and launch, and drives batch execution.
- `resolve` discovers plan files, resolves targets, expands dependency closures, constructs `ForgeExecutionPlan`.  See `src/resolve/`.
- `forge` fetches sources, runs pipeline stages in sandboxes, slices outputs.  See `src/forge/`.
- `seal` validates staging output (FHS, ELF lint), creates `.wright.tar.zst` archives.  See `src/seal/`.
- `deploy` extracts archives, copies files to target root, records in the database, runs hooks.  See `src/transaction/`.  Crash-safe via the `delivery` state machine (`src/delivery/`).

## Responsibilities

### Build-side commands

- `wright resolve` expands dependency and reforge scope.
- `wright build` executes sandboxed stages and writes `staging/` and `outputs/`.
- `wright package` slices staging output and writes `.wright.tar.zst` archives to `parts_dir`.
- `wright prune` removes stale archives.

### `wright`

- resolve local part names by scanning `parts_dir` and reading `.PARTINFO`
- deploy and upgrade archives transactionally
- remove parts and cascade orphan cleanup
- verify and inspect the live system
- run `install` as the high-level convergence operation:
  resolve targets, execute forge waves, and deploy each wave before advancing
- run `launch` to fill a fresh target root from plans or folios, sharing
  the deploy transaction code with the live-system commands

## Shared State

The deployed registry (`wright.db`) records facts about deployed parts —
what they declare, not what is enforced. Runtime dependencies are advisory;
`registered`, `satisfied`, and `runnable` are independent states queried by
different commands. See
[ADR-0016](../adr/0016-advisory-runtime-dependencies.md).

Detailed database schemas and their roles are documented in [Database Design](../reference/database-design.md).

| Artifact | Written by | Read by |
|----------|-----------|---------|
| `plan.toml` | user | `wright build`, `wright resolve`, `wright install` |
| `staging/` | `wright build` | `wright package`, user inspection |
| `.wright.tar.zst` | `wright package`, `wright install` | `wright merge`, `wright upgrade`, `wright install` |
| `store/<hash>-<name>.part` | `wright install` (post-seal) | `wright install` (pre-forge CAS check) |
| `wright.db` | `wright` | `wright`, `wright resolve`, `wright build`, `wright install` |

For recovery from interrupted deliveries, see [Delivery Recovery](delivery-recovery.md).
For build sandboxing, see [Isolation Model](isolation-model.md).
For module-level code organization, see [Module Layout](../dev/module-layout.md).
