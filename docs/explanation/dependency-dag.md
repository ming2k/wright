# Dependency DAG

Wright does not use a single dependency graph for everything.  It maintains **two separate directed acyclic graphs (DAGs)** because build-time ordering and runtime installation ordering solve different problems and can have different shapes.

---

## Overview

| DAG | Lives In | Edges From | Used For |
|-----|----------|-----------|----------|
| **Build Dependency DAG** | `src/resolve/graph.rs` | `build_deps`, `link_deps`, `runtime_deps` | Grouping plans into parallel-safe batches |
| **Runtime Dependency DAG** | `src/transaction/dag.rs` | `runtime_deps` (from archive metadata) | Ordering part installation within a batch |

A build-only dependency (for example a header-only library needed only at compile time) appears in the build DAG but may not need to be deployed before the dependent's runtime files are copied.  Keeping the two DAGs separate lets Wright parallelize compilation as aggressively as possible while still installing runtime dependencies in a safe order.

---

## Build Dependency DAG

### Edge Construction

`build_dep_map` (`src/resolve/graph.rs`) constructs the graph from the current build set:

1. Load every plan manifest in the build set.
2. Build a `part_to_plan` map so that multi-output plans (e.g. `gcc` producing both `gcc` and `libstdc++`) resolve correctly.
3. Collect edges according to the active `DepDomain`:
   - `BUILD` — follows `build_deps`
   - `LINK` — follows `link_deps`
   - `RUNTIME` — follows `runtime_deps`
   - `ALL` — follows all three

Only dependencies that are also in the build set become edges.  A missing dependency does not create an edge; it is handled later by the missing-dependency expansion logic.

### Topological Sort into Batches

`construction_plan_batches` applies Kahn's algorithm:

```rust
while !ready.is_empty() {
    let current_batch = ready;      // all indegree-0 nodes
    ready = Vec::new();
    for name in current_batch {
        for child in dependents[&name] {
            indegree[child] -= 1;
            if indegree[child] == 0 {
                ready.push(child);
            }
        }
    }
    batch += 1;
}
```

All nodes with indegree 0 at the same time form a single batch.  This maximizes parallelism: every task in a batch can be forged simultaneously because none depends on another in the same batch.

If the algorithm finishes without ordering every node, a cycle exists.  Wright reports the cycle nodes and aborts.

### Breaking Cycles with Bootstrap Passes

Some valid toolchains contain cycles (e.g. `gcc` needs `binutils`, which needs `gcc` to build).  Wright does not remove edges; it inserts **bootstrap passes**:

- `collect_phase_deps` computes the full dependency set and, optionally, the MVP (Minimum Viable Product) dependency set.
- Plans that differ between the two sets are flagged as bootstrap candidates.
- `inject_bootstrap_passes` (`src/forge/mvp.rs`) adds tasks like `gcc:bootstrap` to the graph.  The bootstrap pass builds with fewer dependencies, breaking the cycle.  A second full `gcc` build then uses the bootstrapped compiler.

### Rebuild Propagation

`expand_rebuild_deps` propagates rebuild reasons transitively:

- If a plan is rebuilt because its link dependency changed, all plans that transitively depend on it through link or runtime edges may also need rebuilding.
- The algorithm walks the graph wave by wave until no new rebuilds are discovered.
- Stable-toolchain plans can be excluded from transitive rebuilds to avoid blanket rebuild cascades.

---

## Runtime Dependency DAG

### Purpose

When a batch is deployed, the individual parts within that batch may still have runtime dependencies on each other.  The runtime DAG ensures that a part is installed **only after** its runtime dependencies are already present on the live system.

### Construction and Sort

`sort_dependencies` (`src/transaction/dag.rs`) performs a DFS-based topological sort on the `ResolvedPart` metadata extracted from the archives:

```rust
fn visit_resolved(name, map, visited, visiting, sorted) {
    if visiting.contains(name) { return Err(circular); }
    visiting.insert(name);
    for dep in &map[name].dependencies {
        visit_resolved(dep, ...)?;
    }
    visiting.remove(name);
    visited.insert(name);
    sorted.push(name);
}
```

The resulting `Vec<String>` is the safe installation order.  Wright deploys parts in this order within the batch.

### Example

Consider a batch containing `python` and `python-certifi`.  `python-certifi` declares a runtime dependency on `python`.  The build DAG places both in the same batch because `python-certifi` does not need `python` to compile.  The runtime DAG, however, orders `python` before `python-certifi` during deploy so that the interpreter is on disk before the certificate bundle is installed.

---

## Why Two DAGs?

| Concern | Build DAG | Runtime DAG |
|---------|-----------|-------------|
| **Scope** | Plans (source descriptions) | Parts (compiled archives) |
| **Edges** | `build_deps` + `link_deps` + `runtime_deps` | `runtime_deps` only |
| **Goal** | Maximize parallelism, respect compile-time ordering | Ensure runtime libraries exist before dependents need them |
| **Shape** | Can contain cycles (broken by bootstrap) | Must be acyclic; cycles are fatal errors |
| **When used** | Resolve phase, before any compilation | Deploy phase, after all archives are sealed |

Separating them also allows Wright to support **build-only dependencies** — tools or headers required to compile a plan but not needed at runtime.  These appear in the build DAG (affecting batch placement) but never in the runtime DAG (no installation ordering constraint).
