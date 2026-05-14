# Batch Processing

Wright does not build and install plans one at a time, nor does it attempt a single global transaction across an entire tree.  Instead it uses **batches** — groups of tasks that can safely be built in parallel and are sealed and deployed as a single unit.

This document explains how batches are constructed, how they execute, and why the boundaries are drawn the way they are.

---

## What a Batch Is

A **batch** (also called a **wave**) is a set of tasks that share the same topological depth in the dependency graph.  Every task in a batch depends only on tasks from earlier batches, and tasks within the same batch have **no dependencies on each other**.

This means:
- All tasks in a batch can be forged **in parallel**.
- When a batch begins, every dependency it needs is already resolved, forged, sealed, and **deployed** on the live system.
- The batch itself is the smallest atomic deployment unit.

---

## How Batches Are Constructed

Batch construction happens during the resolve phase.

1. **`build_dep_map`** (`src/resolve/graph.rs`) reads each plan's `build_deps`, `link_deps`, and `runtime_deps` and builds a directed graph containing only the plans in the current build set.
2. **`construction_plan_batches`** applies Kahn's algorithm to the graph:
   - Seed the queue with all indegree-0 nodes.
   - Each iteration removes every ready node as a single batch, then decrements the indegree of their dependents.
   - Any node that reaches indegree 0 joins the next batch.
3. If a cycle is detected, Wright injects bootstrap passes (e.g. `gcc:bootstrap`) to break the cycle before full builds proceed.

The result is a `ForgeExecutionPlan` whose `batches` field is a `Vec<Vec<String>>` ordered from roots to leaves.

---

## Execution Flow Inside a Batch

`execute_install` (`src/operations/install.rs`) processes batches sequentially.  Inside each batch the flow is rigid:

### 1. CAS Pre-check

Before any work begins, Wright computes a **closure fingerprint** for each plan (a SHA256 of the plan's build key plus the fingerprints of its direct build dependencies).  If every output part of a plan already exists in the Content-Addressed Store (`store_dir/`), the plan is marked as a CAS hit and its forge and seal steps are skipped.

### 2. Parallel Forge

Every non-CAS task in the batch is spawned as an independent `tokio::task`:

```rust
for task in batch {
    let handle = tokio::spawn(async move {
        forger.build(&manifest, &plan_dir, ..., configure_lock, compile_lock)
            .await
    });
}
```

Each task runs its own complete pipeline:

```
fetch → verify → extract → prepare → configure → compile → check → staging
```

There is **no batch-level barrier** at any stage.  Task A does not wait for Task B to finish extracting before it enters `configure`.  The only cross-task synchronization is resource throttling:

| Lock | Permits | Purpose |
|------|---------|---------|
| `network_pool` | `max_concurrent_downloads` | Throttles concurrent source downloads globally |
| `configure_lock` | 1 | Serializes `configure` stages across all tasks (autotools scripts often race on shared state) |
| `compile_lock` | `total_cpus` | Each `compile` stage claims `effective_cpu` permits; total in-flight compile CPU never exceeds system cores |

### 3. Unified Seal

After **all** forge handles return successfully, Wright seals the batch.  It iterates over the distinct non-bootstrap bases in the batch and calls `package_manifest` for each one.  Bootstrap tasks are skipped because their outputs are intermediate.

Freshly sealed archives are also stored in CAS so that future runs can skip the forge entirely.

### 4. Unified Deploy

Finally, all archive paths (newly sealed + CAS-restored) are collected and deployed in a single call to `deploy_parts_with_explicit_targets`.  The deployer validates the batch as a whole:

- All outputs belonging to the same plan must share the same revision.
- When upgrading a plan, every old output must be replaced; partial upgrades are rejected.

Only after validation succeeds are files copied to the target root and recorded in the database.

---

## Why Seal and Deploy Are Batch-Level

A batch is designed so that every task compiles against the **same system snapshot** — the state left by the previous fully deployed wave.  If Task A were deployed while Task B was still compiling, Task B would observe a partially updated system.  Its `configure` script might detect headers from the new version of Task A, or its linker might resolve symbols differently, producing an inconsistent or non-reproducible build.

By sealing and deploying only after every task in the batch has finished compiling, Wright guarantees:

1. **Compile-time consistency** — all tasks see identical dependency snapshots.
2. **Plan-level atomicity** — multi-output plans are upgraded all at once, never piecemeal.
3. **Simple failure semantics** — if any task fails, nothing from that batch reaches the live system.

---

## Failure Model

### Forge Failure

If any task in a batch fails, the entire `wright install` aborts immediately:

```rust
Ok(Err(e)) => {
    let _ = crate::delivery::rollback_delivery(&db, tx_id).await;
    let _ = crate::delivery::cleanup_delivery(&db, tx_id).await;
    return Err(...);
}
```

The current batch is not deployed.  Earlier batches that were already deployed **remain** on the system.  This is intentional: `wright install` advances the system as far as safely possible, rather than demanding all-or-nothing across the entire tree.

### Deploy Failure

If batch validation or file copy fails, the delivery transaction is rolled back.  Per-part rollback journals (`RollbackState`) restore backups, remove newly created files, and recreate diverted symlinks in reverse order.

---

## Relation to Waves

"Batch" and "wave" are essentially the same concept.  Wright resolves, builds, seals, and deploys **wave 0** completely before starting **wave 1**.  Later waves observe the system as updated by earlier waves.  This is essential for self-hosting scenarios where a newly built compiler must be installed before its dependents can be compiled.

For the original decision, see [ADR-0002: Wave-by-Wave Install](../adr/0002-wave-by-wave-install.md).
