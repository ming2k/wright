---
id: ADR-0047
title: "Bidirectional Subgraph Expansion and Blast Radius Control"
status: accepted
date: 2026-03-30
scope: core/engine
superseded_by: null
negative_knowledge: true
---

# 0047. Bidirectional Subgraph Expansion and Blast Radius Control

- Status: Accepted
- Date: 2026-03-30
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

In [ADR-0046](0046-scoped-dependency-resolution-and-update-containment.md), Wright established scoped dependency resolution by defaulting `dep_match_policies` to `[MatchPolicy::Missing]` during targeted package operations (`install`, `upgrade`). This successfully prevented accidental cascade updates (unintended update propagation of WIP or unchanged dependencies).

However, in real-world maintenance of from-source systems, operators frequently face two essential, intentional dependency graph scenarios:

1. **Upstream Bottom-Up Chain Update (`deps`, target as root)**:
   When maintaining or updating an application or subsystem (e.g., `nginx` or a desktop component), an operator intentionally needs to update its entire forward dependency chain (`target -> dep1 -> dep2 -> ... -> leaf`) bottom-up. Because `upgrade` hardcoded `dep_match_policies: [Missing]`, operators were forced to either manually upgrade dozens of lower-level libraries one-by-one or run `wright upgrade all`, which has an unbounded system-wide blast radius.

2. **1-Hop Impact Radius on Dependents (`rdeps`, 1-layer reverse rebuild)**:
   When modifying or upgrading a core shared library (e.g., `openssl`, `zlib`), rebuilding with unbounded reverse dependency expansion (`depth=0`) triggers a catastrophic "rebuild the world" storm involving hundreds of unrelated packages. Conversely, skipping reverse rebuilds altogether (`rdeps=none`) risks ABI breakage in direct consumers. The critical engineering sweet spot is **updating exactly one layer upward** (`rdeps_depth=1`, direct consumers/dependents), capturing direct link/runtime dependents while strictly containing the blast radius.

3. **Multiplexed Depth Flaw in Engine Options**:
   In `ResolveOptions`, a single `depth: Option<usize>` field was multiplexed across both forward dependency expansion (`deps`) and reverse dependent expansion (`rdeps`). Consequently, it was architecturally impossible to express: *"expand the entire bottom-up upstream dependency chain (unlimited depth) while restricting reverse dependents to exactly 1 hop (depth=1)"*. Setting `depth=1` truncated the upstream chain prematurely; setting `depth=0` caused an unbounded reverse rebuild storm.

## Decision Drivers

- **Zero Legacy Compromise**: Eliminate multiplexed depth parameters; provide completely orthogonal, first-class directional control.
- **Explicit Operator Intent**: Respect ADR-0046's default containment while providing first-class CLI mechanisms (`--deep`, `--rdeps-depth=1`) for intentional graph updates.
- **Strict Blast Radius Bounding**: Enable precise 1-hop containment for reverse dependents, fully composable with ADR-0045 deterministic ABI probe inhibition.
- **Observability & Determinism**: Allow operators to inspect both upstream build chains and downstream impact layers via `wright resolve --tree`.

## Considered Options

- **Option 1: Decoupled Directional Depths & First-Class `--deep` Subgraph Expansion (Chosen)**
  - Decouple `depth` in `ResolveOptions` into independent `deps_depth: Option<usize>` and `rdeps_depth: Option<usize>`.
  - Introduce `--deep` (aliased as `--upgrade-deps`) on `upgrade` and `install` to elevate forward dependency policy from `Missing` to `Outdated` (or `All` under `--force`).
  - Introduce explicit reverse-depth bounding via `--rdeps-depth <N>` (with `--rdeps-depth=1` representing the 1-hop direct dependent sweet spot).
- **Option 2: Status Quo with Multiplexed `--depth`**
  - Keep a single `depth` for both directions. (Discarded: cannot simultaneously traverse full upstream chain while bounding downstream blast radius).
- **Option 3: External Scripting / Plumbing Pipes**
  - Require operators to pipe `wright resolve --tree` through custom awk/shell scripts to construct targeted build batches. (Discarded: poor ergonomics, non-atomic resolution, error-prone).

## Decision Outcome

Chosen option: **Option 1: Decoupled Directional Depths & First-Class `--deep` Subgraph Expansion**.

### Architectural Changes

1. **Decoupled Resolution Options (`ResolveOptions`)**:
   `ResolveOptions` replaces the single `depth` field with separate, non-interfering directional depth bounds:
   ```rust
   pub struct ResolveOptions {
       pub deps: DepDomain,
       pub rdeps: DepDomain,
       pub match_policies: Vec<MatchPolicy>,
       pub dep_match_policies: Option<Vec<MatchPolicy>>,
       pub deps_depth: Option<usize>,   // Forward dependency traversal depth (0 / None = unlimited)
       pub rdeps_depth: Option<usize>,  // Reverse dependent traversal depth (1 = direct consumers only)
       pub include_targets: bool,
       pub preserve_targets: bool,
   }
   ```

2. **Graph Expansion Engine Separation**:
   In `crates/wright-resolve/src/lib.rs` and `graph.rs`:
   - `expand_missing_dependencies` evaluates depth strictly against `deps_depth`.
   - `expand_rebuild_deps` evaluates depth strictly against `rdeps_depth`.
   - A depth value of `0` or `None` signifies unbounded traversal (`usize::MAX`), while `Some(N)` bounds traversal to $N$ hops.

3. **CLI Porcelain Additions**:
   - `wright upgrade <target> --deep`:
     Sets `dep_match_policies = Some(vec![MatchPolicy::Outdated])`, recursively updating outdated upstream dependencies bottom-up towards `<target>`.
   - `wright upgrade <target> --rdeps-depth=1` (or `--depth=1`):
     Limits reverse dependency rebuilds to direct consumers (1-hop blast radius).
   - `wright upgrade <target> --deep --rdeps-depth=1`:
     Simultaneously performs bottom-up upstream chain update and 1-hop downstream consumer alignment.
   - `wright install <target> --deep [--force]`:
     When passed with `--force`, recursively rebuilds `<target>` and its entire forward dependency subgraph from scratch (`dep_match_policies = Some(vec![MatchPolicy::All])`).
   - `wright resolve <target> --deep --rdeps-depth=1 --tree`:
     Renders the exact bidirectional execution forest showing upstream build chains and 1-hop downstream impact.

4. **Integration with ADR-0045 ABI Circuit Breaker**:
   When `rdeps_depth = Some(1)` pulls in direct consumers, the physical ELF ABI probe from ADR-0045 remains fully active. If the updated target library maintains backward ABI compatibility, the 1-hop rebuilds are inhibited unless `--no-inhibit-rebuild` or `--force` is specified.

### Positive Consequences

- **Ergonomic Full-Stack Iteration**: Maintainers can refresh or rebuild an entire component stack without touching unrelated parts of the system.
- **Predictable Blast Radius**: Operators can update shared libraries with the confidence that only 1-hop direct consumers will be rebuilt, eliminating runaway world-rebuilds.
- **Architectural Purity**: Fully removes the multiplexing hack in `ResolveOptions` without compromising the isolation guarantees of ADR-0046.

### Negative Consequences

- Running `--deep` on a package with a deep, complex dependency graph (e.g. `gcc` or `mesa`) may trigger significant compilation if multiple upstream dependencies have local plan edits.

---

## Rejected Alternatives & Negative Knowledge

### Why Multiplexed Depth (Option 2) Was Discarded

Multiplexing a single depth counter across two topological directions with inverse engineering goals was a fundamental design flaw:
- Forward dependencies represent *requirements* (needed to manufacture the target). Operators almost always want either 0 (none) or $\infty$ (complete bottom-up satisfaction).
- Reverse dependents represent *impact / blast radius* (who might be broken by the target). Operators almost always want either 0, 1 (direct dependents), or selectively 2.
Forcing both to share one parameter made either the forward chain incomplete or the reverse chain uncontrollable.
