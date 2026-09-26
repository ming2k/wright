---
id: ADR-0046
title: "Scoped dependency resolution and update containment"
status: accepted
date: 2026-03-30
scope: core/engine
superseded_by: null
negative_knowledge: true
---

# 0046. Scoped dependency resolution and update containment

- Status: Accepted
- Date: 2026-03-30
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

In [ADR-0003](0003-default-resolution-policy.md), Wright established a default dependency expansion policy of `--deps --match=outdated`. The rationale was "source-first convergence": automatically adding missing and outdated dependencies during package operations.

In practice, this caused severe **unintended update propagation (cascade drift)**:
When an operator ran targeted commands (`wright install <target>` or `wright upgrade <target>`), the resolver recursively inspected all forward dependencies. If any installed dependency happened to have a newer version in the local plans repository, the resolver marked it as `outdated` and added it to the build set. If that dependency had outdated dependencies, the update cascade continued recursively through the graph.

This behavior produced several critical defects:
1. **Violation of Locality & Principle of Least Surprise**: Operators attempting to update a single package (`optics`) observed unrelated dependencies being unexpectedly upgraded.
2. **Leakage of Uncommitted / WIP Plans**: A maintainer editing an unrelated library in `plans/` would find their draft code inadvertently compiled and deployed into production when installing a completely different package that depended on that library.
3. **Semantic Conflation of Target Policy and Dependency Policy**: `ResolveOptions` used a single `match_policies` field for both explicit targets and automatically expanded dependencies.

## Decision Drivers

- **Strict Locality of Change**: Targeted operations must affect only the requested targets, with zero spontaneous side-effects.
- **Bounded Blast Radius**: Forward dependencies must be installed only when genuinely needed to build or run the target.
- **Explicit Operator Intent**: Global or eager dependency upgrades must occur only upon explicit user request.
- **Clean Separation of Concerns**: Decouple the matching policy for explicit targets from the policy for forward-expanded dependencies.

## Considered Options

- **Option 1: Bifurcated Target & Dependency Resolution Policies (Chosen)**
  - Decouple target matching from dependency matching in `ResolveOptions`.
  - Targeted commands default `dep_match_policies` to `[MatchPolicy::Missing]`.
  - Satisfied dependencies are left untouched.
- **Option 2: Status Quo (ADR-0003 Eager Outdated Convergence)**
  - Keep auto-upgrading all outdated dependencies. (Discarded: causes unbounded update propagation).
- **Option 3: Strict Lockfiles (Cargo.lock-style mandatory pinning)**
  - Require a global lockfile of all deployed package hashes. (Discarded: excessive ceremony for a source-first system package manager).

## Decision Outcome

Chosen option: **Option 1: Bifurcated Target & Dependency Resolution Policies**.

### Architectural Changes

1. **Bifurcated Resolution Options (`ResolveOptions`)**:
   `ResolveOptions` now explicitly distinguishes target policies from dependency policies:
   ```rust
   pub struct ResolveOptions {
       pub deps: DepDomain,
       pub rdeps: DepDomain,
       pub match_policies: Vec<MatchPolicy>,
       pub dep_match_policies: Option<Vec<MatchPolicy>>,
       pub depth: Option<usize>,
       pub include_targets: bool,
       pub preserve_targets: bool,
   }
   ```
2. **Default Dependency Expansion Policy (`MatchPolicy::Missing`)**:
   - For targeted operations (`install`, `upgrade`), `dep_match_policies` defaults to `vec![MatchPolicy::Missing]`.
   - Forward dependencies are only added to the build set if they are **missing** from the host system.
   - Satisfied, deployed dependencies are never rebuilt or upgraded.
3. **Explicit Operator Overrides**:
   - Operators can explicitly request eager upgrades by passing `--match=outdated` on the CLI or invoking global operations (`wright upgrade all`).
   - When `--match` is explicitly provided, both target policies and dependency policies respect the operator's choice.

### Positive Consequences

- **Zero Accidental Update Propagation**: Targeting package `A` touches only `A` (plus any strictly missing dependencies).
- **Draft & WIP Safety**: Local work-in-progress edits in `plans/` cannot accidentally leak into deployments of unrelated packages.
- **Alignment with Industry Standards**: Matches the well-established semantics of `pacman`, `apt`, `cargo`, and `nix`.

### Negative Consequences

- When an existing dependency is deployed but buggy, installing a new package that depends on it will not automatically upgrade that dependency unless the operator explicitly passes `-m outdated` or updates the dependency directly.

---

## Rejected Alternatives & Negative Knowledge

### Why Option 2 (ADR-0003 Eager Outdated Convergence) Was Discarded

Eager outdated convergence assumed that the local plan repository always represents a desired unified system state. In real-world multi-package repositories, plans are modified independently. Treating every dependency edge as an automatic upgrade vector turned every local edit into a global liability, destroying predictability and reproducibility.
