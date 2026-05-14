# Execution Hierarchy

Wright uses a three-tier metaphor for the journey of source into a running
system.  Understanding these tiers clarifies how plans, build steps, and
individual stages relate to each other.

## Why Three Tiers?

A plan's full build spans too many levels of abstraction to describe with
a single term.  Collapsing everything into "build" or "install" muddies the
distinction between strategy (what should happen) and mechanism (how each step
executes).  Three tiers give each scale its own name and responsibility.

## Tier 1 (Macro): Delivery

A **Delivery** is the complete arc of a plan: **resolve → build → seal → deploy**.

Every plan that participates in a `wright install` goes through this
four-step journey.  The user's intent is a delivery — "get GCC onto this
system" — and Wright's job is to execute the resolve, build, seal, and deploy
steps needed to fulfill that delivery.

| Step | What happens | Code home |
|------|-------------|-----------|
| **Resolve** | Discover plan files, build name → path index, resolve targets to canonical `plan.toml` paths | `src/resolve/resolver.rs`, `src/plan/discovery.rs` |
| **Build** | Fetch sources, run forge stages, slice outputs (orchestrated by the Foundry) | `src/foundry/`, `Foundry::build` |
| **Seal** | FHS-validate output directories, ELF-lint runtime dependencies, create `.wright.tar.zst` archive | `src/seal/`, `src/part/` |
| **Deploy** | Extract archive, detect conflicts, copy files to target root, record in `wright.db`, run hooks | `src/transaction/` |

In a `wright install`, deliveries are grouped into **waves** by topological
dependency sorting.  Every plan in a wave depends only on plans from earlier
waves.  Wright resolves, builds, seals, and deploys one complete wave before
starting the next, ensuring that when a plan's `configure` stage needs a
library header, that library's files are already present in the system root.

## Tier 2 (Micro): Foundry

The **Foundry** is the workshop inside the **build** step.  It orchestrates
three subsystems that transform raw materials into shaped artifacts:

| Subsystem | Stages | What happens | Metaphor |
|-----------|--------|-------------|----------|
| **Charge** | `fetch → verify → extract` | Source preparation: procure raw materials, assay purity, break them down | Loading ore into the furnace |
| **Forge** | `prepare → configure → compile → check → staging` | Build execution: heat, hammer, and shape source through transformative stages | The core smithing process |
| **Mold** | `slice` | Output slicing: pour the forged artifact into molds to produce distinct, named outputs based on `[[output]]` rules | Casting into final form |

Each subsystem operates on its own stages:

- **Charge stages** (`fetch`, `verify`, `extract`) are built-in — handled by
  `Charge::prepare` directly, not by user-defined scripts.  They run before
  the forge stages and can be targeted with `--until-stage`.

- **Forge stages** (`prepare`, `configure`, `compile`, `check`, `staging`) are
  user-defined — each declared by a `[pipeline.<name>]` block in `plan.toml`.
  They run inside an optional sandbox with pre- and post-hooks.

- **Mold stage** (`slice`) is a single built-in operation that distributes
  files from `staging/` into `outputs/<name>/` per the plan's `[[output]]`
  rules.  Mold is the **only** subsystem responsible for output division;
  Seal never performs slicing.

The `Forge` engine (inside the Foundry) uses **OverlayFS layers** and
**hash-chain checkpoints** for incremental builds.  Each stage's input hash
is chained to its predecessor: a change to any upstream script or environment
variable automatically invalidates all downstream stages.  Checkpoint state
is stored in `.wright-pipeline.json` in the build root.

## Tier 3 (Atomic): Stage

A **Stage** is the smallest unit of work — a single script execution or
built-in operation within a Foundry subsystem.

Each forge stage declared in `plan.toml` can specify:

| Field | Purpose |
|-------|---------|
| `script` | Shell commands to execute |
| `executor` | Which executor runs it (defaults to `bash`) |
| `isolation` | Sandbox level: `none`, `relaxed`, or `strict` |
| `env` | Per-stage environment variables |

Stages support hooks: `pre_<stage>` runs before the stage script, and
`post_<stage>` runs after.  Both hooks live in the same `[pipeline]`
namespace.

Stage execution in code is handled by `Forge::run`
(`src/foundry/forge.rs`).  The forge runner determines the effective CPU
count (when locks are held), applies variable substitution, logs to
`logs/<stage>.log`, and retries on ETXTBSY races.

## How the Tiers Map to Code

| Tier | Term | Key Types | Module |
|------|------|-----------|--------|
| Macro | Delivery | orchestration across `cli/` → `operations/` → `resolve/` → `foundry/` → `transaction/` | — (cross-cutting) |
| Micro | Foundry | `Foundry`, `Charge`, `Forge`, `Mold` | `src/foundry/` |
| Atomic | Stage | `ForgeContext`, stage runner | `src/foundry/forge.rs`, `src/foundry/executor.rs` |

## Relationship to CLI Commands

| Command | Tier engaged |
|---------|-------------|
| `wright build` | Build step only (Foundry: Charge + Forge + Mold) |
| `wright merge` | Deploy (from existing archive) |
| `wright install` | Full Delivery (resolve + build + seal + deploy, wave by wave) |
| `wright launch` | Full Delivery on a fresh root |
