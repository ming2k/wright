# `wright apply` Design

## Purpose

`wright apply` is not just a thin wrapper around `wright build` and `wright install`.
It is Wright's high-level convergence command: given plan names, plan paths, or
`@assembly` targets, it moves the live system toward the state described by the
current plan tree.

The lower-level commands remain important:

- `wright build` manufactures archives from plans.
- `wright resolve` expands a build set.
- `wright install` applies chosen archives to the live system.

`wright apply` exists because day-to-day maintenance usually wants one higher
level policy:

- start from plans, not from archive filenames
- reuse local archives when they are already valid
- add missing or outdated upstream plans when they are required
- install and upgrade in dependency waves so later work sees the updated system

That policy is opinionated by design. The command is meant to feel "smart"
without becoming magical.

## Command Contract

`wright apply <TARGET...>` accepts:

- plan names
- plan directories
- `@assembly` references
- targets from stdin when not attached to a TTY

Its job is to converge those requested targets onto the live system by:

1. resolving the requested plans
2. expanding the necessary upstream build set
3. creating a dependency-ordered execution plan
4. building each dependency wave
5. installing each completed wave before moving to the next one

This is a source-first workflow. The user asks for desired parts or assemblies,
not for a precomputed archive list.

## Smart Defaults

The current implementation encodes a small default policy rather than asking
the user to assemble dependency expansion by hand on every run.

### Default Resolution Policy

When the user does not pass explicit dependency flags, `wright apply`
currently resolves its build set as if it had called:

```bash
wright resolve <targets> --deps --match=outdated
```

More precisely:

- explicit targets are included when they are missing or differ from the installed plan state
- upstream dependencies are expanded across build, link, and runtime edges
- missing and outdated upstream dependencies are auto-added by default
- reverse dependent expansion is disabled by default
- depth is unlimited for the default upstream traversal

This default is deliberate.

Adding missing and outdated upstream plans is the minimum useful "smart"
behavior for a source-first convergence command. If the user asks Wright to
apply a target and some prerequisites are absent or no longer match the plan
tree, Wright should pull those plans into the build graph automatically instead
of requiring a separate manual resolve step.

At the same time, `apply` does **not** default to reverse rebuild cascades.
Rebuilding downstream dependents is a heavier policy decision and remains an
explicit low-level workflow through `wright resolve --rdeps`.

### Inventory-First, Plan-Driven

`wright apply` does not blindly rebuild everything.

- If the local inventory already contains matching build outputs, they can be reused.
- If an upstream part is missing or outdated and a plan exists for it, Wright builds it.
- The install step still resolves archive dependencies from the local inventory.

This makes `apply` neither purely build-first nor purely install-first. It is a
coordinated plan-to-system command.

### Separate Force Controls

`wright apply` uses a unified force mechanism:

- `-f`, `--force` forces a clean rebuild (clears the per-plan build workspace and build cache, but keeps downloaded source cache) and a re-installation of the resulting parts.

This consolidates the previous separate flags into a single control for situations where you want to ensure the system is completely refreshed from the plan state.


## Execution Model

The implementation in `src/commands/system.rs` follows this pipeline.

### 1. Determine Explicit Targets

Before dependency expansion, `apply` resolves the user's original targets to
canonical plan names.

This information is used later for install-origin tracking:

- parts explicitly requested by the user become `manual`
- parts pulled in automatically become `dependency`

That distinction is preserved even when one plan produces multiple output parts.

### 2. Build a Resolved Plan Set

`apply` computes a build set with:

- `deps = all`
- `match = outdated`
- `DependentsMode::None`
- `include_targets = true`

This is the core "smart default" layer: enough expansion to keep requested
targets and prerequisites converged to the current plan state, without silently
turning every maintenance run into a blanket rebuild policy.

### 3. Create a Wave Plan

The resolved plan names are converted into a `BuildExecutionPlan`.

That plan groups tasks into dependency-safe batches. Each batch contains only
tasks that can be built before the next dependency level begins.

The same machinery also carries build labels such as:

- `build`
- `relink`
- `build:mvp`
- `build:full`

So `apply` inherits the normal Wright build scheduler instead of inventing a
second orchestration system.

### 4. Optional Dry Run

With `--dry-run`, `apply` stops after planning and prints:

- batch number
- build label
- base plan name
- whether the resulting install origin is `explicit` or `dep`

Dry-run is therefore a plan inspection tool, not only a yes/no preview.

### 5. Build and Install Per Wave

For every batch:

1. build the batch
2. discover the archive outputs for each plan
3. deduplicate archive paths within the batch
4. install that batch onto the live system

This wave-by-wave installation model is the defining behavior of `apply`.

It means later waves observe the system as updated by earlier waves. That is
important for self-hosting and rolling maintenance workflows where newly built
toolchain or library parts should be visible before building their dependents.

## Why Wave-by-Wave Install Instead of One Big Install

This command is specifically designed for organic system evolution, not only for
archive production.

The wave model gives Wright several useful properties:

- dependency order remains explicit and inspectable
- earlier rebuilt foundations can affect later builds
- the live system is updated progressively rather than only at the very end
- the command maps naturally onto assemblies and source-first maintenance

The tradeoff is that `apply` is not a single global transaction across every
wave. Each install batch succeeds or fails in sequence.

That tradeoff is intentional. For this command, correctness of staged system
convergence is more important than pretending the entire multi-wave operation is
one atomic install.

## Failure Model

`wright apply` is resilient, but not globally atomic.

If a later batch fails:

- earlier successful install waves remain applied
- Wright prints a note listing parts already installed in previous batches
- the operator can fix the issue and rerun `wright apply`

This behavior matches the command's role as a maintenance orchestrator. It is
closer to "advance the system safely as far as possible" than to "all or
nothing from first plan to last plan".

## Relation to Install Origins

`apply` must preserve user intent, not just dependency closure.

If the user asks for `wright apply gcc`, then:

- `gcc` should be treated as manually requested
- anything pulled in only because `gcc` needs it should be marked as dependency-installed

This is why `apply` first records the explicit plan names before dependency
expansion. Without that step, a smart maintenance command would blur the
difference between "I asked for this" and "Wright needed this".

That distinction feeds later behaviors such as orphan cleanup and origin
promotion.

## What `apply` Is Not

`wright apply` is intentionally not:

- a replacement for `wright resolve --rdeps` deep rebuild workflows
- a hidden alias for `wright build && wright install`
- a global rollback boundary across all dependency waves
- a fully user-programmable policy engine

The command is high-level, but still constrained. When operators want a
different rebuild policy, they should drop to the lower-level pipeline
explicitly.

## Recommended Mental Model

Use this rule of thumb:

- use `wright build` when you want archives
- use `wright resolve` when you want to shape rebuild scope
- use `wright install` when you already know which archives to apply
- use `wright apply` when you want the live system to converge toward the
 current plans with Wright's default install/upgrade/dependency combo policy

That last item is the key design point. `apply` is not merely a convenience
command. It is the policy-bearing command in Wright's source-first workflow.
