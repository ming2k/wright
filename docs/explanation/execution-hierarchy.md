# Execution Hierarchy

Wright uses a three-tier metaphor for the journey of source into a running
system.  Understanding these tiers clarifies how plans, pipelines, and
individual build actions relate to each other.

## Why Three Tiers?

A plan's full pipeline spans too many levels of abstraction to describe with
a single term.  Collapsing everything into "build" or "install" muddies the
distinction between strategy (what should happen) and mechanism (how each step
executes).  Three tiers give each scale its own name and responsibility.

## Tier 1 (Macro): Delivery

A **Delivery** is the complete arc of a plan: **resolve → forge → seal → deploy**.

Every plan that participates in a `wright install` goes through this
four-step journey.  The user's intent is a delivery — "get GCC onto this
system" — and Wright's job is to execute the resolve, forge, seal, and deploy
steps needed to fulfill that delivery.

| Step | What happens | Code home |
|------|-------------|-----------|
| **Resolve** | Discover plan files, build name → path index, resolve targets to canonical `plan.toml` paths | `src/resolve/resolver.rs`, `src/plan/discovery.rs` |
| **Forge** | Fetch sources, verify checksums, extract, run pipeline stages, slice outputs | `src/forge/`, `Forger::build` |
| **Seal** | FHS-validate staging output, ELF-lint runtime dependencies, create `.wright.tar.zst` archive | `src/seal/`, `src/part/` |
| **Deploy** | Extract archive, detect conflicts, copy files to target root, record in `wright.db`, run hooks | `src/transaction/` |

In a `wright install`, deliveries are grouped into **waves** by topological
dependency sorting.  Every plan in a wave depends only on plans from earlier
waves.  Wright resolves, forges, seals, and deploys one complete wave before
starting the next, ensuring that when a plan's `configure` stage needs a
library header, that library's files are already on the target root.

## Tier 2 (Micro): Pipeline

A **Pipeline** is the ordered sequence of **Stages** that implements the forge
step of a delivery.  Think of it as the assembly line inside the forge.

The default pipeline is:

```
fetch → verify → extract → prepare → configure → compile → check → staging
```

- `fetch`, `verify`, `extract` are **built-in stages** — handled by the
  `Forger` directly, not by user-defined scripts.  They cannot be targeted
  with `--stage`.

- `prepare`, `configure`, `compile`, `check`, `staging` are **pipeline
  stages** — each defined by a `[pipeline.<name>]` block in `plan.toml`.
  They run user-provided scripts inside an optional sandbox.

Plans may override the pipeline order via `pipeline_order.stages` in
`plan.toml`, or specify a different order for the MVP pass via
`[mvp].pipeline_order`.  The `Pipeline` struct in
`src/forge/pipeline.rs` resolves the effective order per build phase.

Pipelines support **hash-chain checkpoint resume** (see [Checkpoint Recovery](checkpoint-recovery.md)).
Each stage's input hash is chained to its predecessor: a change to any upstream
script or environment variable automatically invalidates all downstream stages.
Checkpoint state is stored in `.wright-pipeline.json` in the build root; there
are no per-stage sentinel files.  The `--force-stage` flag overrides a single
stage's checkpoint, while `--reforge` bypasses all checkpoints.

## Tier 3 (Atomic): Stage

A **Stage** is a single script execution — the smallest schedulable unit of
work in Wright.

Each stage declared in `plan.toml` can specify:

| Field | Purpose |
|-------|---------|
| `script` | Shell commands to execute |
| `executor` | Which executor runs it (defaults to `bash`) |
| `isolation` | Sandbox level: `none`, `relaxed`, or `strict` |
| `env` | Per-stage environment variables |

Stages support hooks: `pre_<stage>` runs before the stage script, and
`post_<stage>` runs after.  Both hooks live in the same `[pipeline]`
namespace.

Stage execution in code is handled by `Pipeline::run_stage`
(`src/forge/pipeline.rs`).  The pipeline runner determines the effective CPU
count (when locks are held), applies variable substitution, logs to
`logs/<stage>.log`, and retries on ETXTBSY races.

## How the Tiers Map to Code

| Tier | Term | Key Types | Module |
|------|------|-----------|--------|
| Macro | Delivery | orchestration across `commands/` → `operations/` → `planning/` → `forge/` → `transaction/` | — (cross-cutting) |
| Micro | Pipeline | `Pipeline`, `PipelineContext`, `DEFAULT_STAGES` | `src/forge/pipeline.rs` |
| Atomic | Stage | `PipelineStage`, `ExecutorOptions` | `src/forge/pipeline.rs`, `src/forge/executor.rs` |

## Relationship to CLI Commands

| Command | Tier engaged |
|---------|-------------|
| `wright build` | Forge only (one pipeline) |
| `wright merge` | Deploy (from existing archive) |
| `wright install` | Full Delivery (resolve + forge + seal + deploy, wave by wave) |
| `wright launch` | Full Delivery on a fresh root |
