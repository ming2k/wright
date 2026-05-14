# Terminology

Wright uses the Ship of Theseus metaphor: the ship keeps sailing while its parts are replaced.

## Core Terms

| Term | Definition |
|------|------------|
| **Plan** | A `plan.toml` build definition. Describes how to fetch, build, and produce one or more parts. |
| **Part** | A built `.wright.tar.zst` archive. The installable unit. |
| **System** | The live machine under management, tracked in `wright.db`. |
| **Output** | A named sub-part produced by a single plan. A plan can declare multiple outputs (e.g. `gcc` and `libstdc++` from one build). |
| **Assembly** | An informal grouping of plans (a directory of plan directories) processed together by `wright install` or `wright build`. |
| **Pack** | A `.wright.pack.tar` artifact bundling a `pack.toml` manifest, the part archives it references, and an optional `overlay/` configuration tree. Superseded by the **folio** manifest; retained for backward compatibility only. |
| **Launch** | The act of converging a target root from a folio manifest or from plan names, performed by `wright launch`. The target gets its own `wright.db`, plan tree, and `wright.toml`, and is fully self-contained. |
| **Overlay** | An optional `/-rooted` tree that used to ship inside a pack for base config like `/etc/hostname` and `/etc/fstab`. In the folio era, post-install config is handled by the folio's `[config]` block instead. |

## Dependency Terms

| Term | Definition |
|------|------------|
| **`build_deps`** | Tools and headers needed during compilation. Mounted into the isolation environment at build time only. Not persisted in the installed registry. |
| **`link_deps`** | ABI-sensitive shared libraries linked by the built binary. Trigger reverse rebuilds when they change. Not persisted in the installed registry. |
| **`runtime_deps`** | Parts required for this part to function after installation. Declared per-output. Recorded in the installed registry as advisory facts; missing targets are surfaced by `wright check` but do not block install. |
| **`replaces`** | Names of parts this part supersedes. Used by `wright install`/`wright upgrade` to migrate references after a rename or split. |
| **`conflicts`** | Names of parts that cannot coexist with this part. Hard install-time constraint; install is rejected unless `--force`. |

## Dependency States

Each registered part is independently characterized by these three states.
A part can be `registered` without being `satisfied`, and `satisfied`
without being `runnable`. See
[Dependency Philosophy](../explanation/dependency-philosophy.md).

| State | Meaning |
|-------|---------|
| **registered** | The part exists in `wright.db` and its files are on disk. |
| **satisfied** | Every entry in the part's `runtime_deps` resolves to another registered part (directly or via `replaces`). |
| **runnable** | The part actually executes — every dynamic-loader request, dlopen target, and data file is present. |

## Origin Values

The `origin` field on an installed part records how it entered the system.

| Origin | Meaning |
|--------|---------|
| `dependency` | Pulled in automatically to satisfy another part's dependency. Eligible for orphan cleanup via `wright remove --cascade`. |
| `build` | Installed as part of a build wave by `wright install`. |
| `manual` | Explicitly requested by the user. Never auto-removable. |
| `external` | Declared as provided by the host system via `wright provide`. Has no filesystem footprint; used to satisfy dependency checks during bootstrap. |

Origins follow a promotion hierarchy: `dependency < build < manual`. Wright
automatically promotes an origin when you explicitly install a part that was
previously pulled in at a lower tier. `external` is managed exclusively via
`wright provide` / `wright remove`.

## Execution Metaphor

Wright uses a three-tier conceptual metaphor to describe the journey of source
code into a running system.  Each tier corresponds to a different scale of
abstraction.  See [Execution Hierarchy](../explanation/execution-hierarchy.md)
for a full discussion.

| Abstract Tier | Term | Contains | Metaphor |
|---------------|------|----------|----------|
| Macro | **Delivery** | resolve → forge → seal → deploy | The grand journey of an artifact from source code to a live, installed part. |
| Micro | **Pipeline** | fetch → configure → compile | An automated assembly line that transforms source into build output. |
| Atomic | **Stage** | e.g. `compile` | A single workstation on that line — one script, one purpose. |

- A **Delivery** is the complete lifecycle of a plan.  A plan is first **resolved**
  (targets discovered from the plan index, converted to canonical `plan.toml` paths),
  then **forged**
  (sources fetched, pipeline stages executed, outputs sliced), then **sealed**
  (FHS-validated, ELF-linted, packed into a `.wright.tar.zst` archive), and
  finally **deployed** (extracted onto the target root, recorded in `wright.db`).
  Deployments use a temporary **WAL** (Write-Ahead Log) for crash recovery and a
  permanent **History** table for auditing.
  Commands like `wright install` orchestrate many deliveries in dependency-safe
  waves.

- A **Pipeline** is the ordered sequence of stages that constitute the forge
  step of a delivery.  The default pipeline runs `fetch`, `verify`, `extract`,
  `prepare`, `configure`, `compile`, `check`, and `staging`.  Plans may declare
  a custom pipeline order via `pipeline_order` or per-MVP-phase ordering.

- A **Stage** is the smallest unit of work — a single script fragment declared
  in `plan.toml` under `[pipeline.<name>]`.  Each stage runs in an optional
  sandbox with pre- and post-hooks (`pre_<stage>`, `post_<stage>`).  Stages
  support checkpoint-based resume: a completed stage is not re-run on retry
  unless `--force-stage` is used.

## Build Terms

| Term | Definition |
|------|------------|
| **MVP build** | A reduced first-pass **pipeline** run that excludes certain dependencies to break a cycle. Defined by `mvp.toml` alongside `plan.toml`. |
| **Full build** | The second pass after an MVP build; runs with all dependencies restored. |
| **Isolation** | A sandboxed environment for running pipeline stages. Levels: `none`, `relaxed`, `strict`. |
| **Sysroot** | A read-only copy of the host's `/usr`, `/bin`, and `/lib` trees used as the root for `strict`-isolation builds. |

## Writing Guidance

- Say **plan** for build definitions, not "package", "formula", or "recipe".
- Say **part** for built archives, not "package" or "binary".
- Say **system** for the live machine being modified, not "host" or "target".
- Say **output** when referring to a specific named sub-part from a multi-output plan.
