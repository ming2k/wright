# Execution Hierarchy

This document defines the technical terminology and structural layers used in Wright's execution model. It clarifies the relationship between high-level user intent and low-level system actions.

## The Four Layers of Execution

Wright organizes execution into four distinct layers of granularity. The scheduling logic (Batch) remains decoupled from domain-specific execution logic (Pipeline).

| Layer | Term | Definition | Scope | Owner |
| :--- | :--- | :--- | :--- | :--- |
| **L1** | **Operation** | A top-level user action initiated via the CLI. | Multi-Plan / System-wide | `src/cli` |
| **L2** | **Batch** | A topologically sorted sequence of plan groups executed concurrently within dependency constraints. | Inter-Plan Dependencies | `src/operations/drive.rs` |
| **L3** | **Pipeline** | The internal **logic lifecycle** of a plan build or package. | Domain-specific flow | `src/builder/lifecycle.rs` |
| **L4** | **Stage** | The minimum unit of **atomic execution**. Usually a single script or command. | Sandbox / Process | `src/builder/executor` |

---

## Structural Visualization

The following diagram illustrates how these layers nest within each other.

```mermaid
graph TD
    subgraph L1_Operation [L1: Operation]
        direction TB
        L2_Batch[L2: Batch Driver]
    end

    subgraph L2_Batch [L2: Batch]
        direction LR
        GroupA[Batch 0: glibc, linux-headers] --> GroupB[Batch 1: gcc, binutils]
        GroupB --> GroupC[Batch 2: curl]
    end

    subgraph L3_Pipeline [L3: Pipeline Detail]
        direction TB
        Execute[execute method] --> L4_Pipeline[L4: Lifecycle Pipeline]
    end

    subgraph L4_Pipeline [L4: Build Pipeline]
        direction LR
        S1[L5: Stage - fetch] --> S2[L5: Stage - configure]
        S2 --> S3[L5: Stage - compile]
        S3 --> S4[L5: Stage - staging]
    end

    GroupB -.-> L3_Pipeline
```

---

## Conceptual Separation: Scheduling vs. Execution

A critical architectural boundary exists between **Batch (L2)** and **Pipeline (L3)**.

### The Scheduling Layer (Batch Driver)
The batch driver is "blind" to the internals of a plan build. It only cares about:
- **Prerequisites:** "Have all dependency plans in earlier batches completed?"
- **Resources:** "Is there a CPU or RootMutator lock available?"
- **State:** "Did the plan build succeed or fail?"

### The Execution Layer (Pipeline & Stage)
The Pipeline is the domain-specific "worker" inside each batch item. For a build:
- **Context:** It manages a shared `/work` directory and isolation environment across its Stages.
- **Order:** It knows that `compile` must follow `configure`.
- **Granularity:** It uses **Checkpoints** to skip already completed Stages, but these checkpoints are internal to the plan and not managed by the global batch driver.

### Two-Level Recovery (Orthogonal Caches)

Wright maintains **two independent levels of execution progress**. They do not invalidate each other.

| Level | Storage | Granularity | Managed By | Purpose |
|-------|---------|-------------|------------|---------|
| **Batch State** | In-memory (`FuturesUnordered`) | Plan (L2) | `operations/drive.rs` | "Which plans have been built/packaged/installed in this run?" |
| **Stage Checkpoints** | `.wright-stage-*` sentinel files in `work/` | Stage (L4) | `builder/checkpoint.rs` | "Which lifecycle stages inside this plan are done?" |

**Key insight:** Stage Checkpoints survive across command invocations. A restarted `build` can still skip `configure` if its sentinel is valid. To force a full rebuild, use `--force` (which ignores Stage Checkpoints) or remove the `work/` directory.

### Content-Addressed Checkpoints

Stage Checkpoints are **content-addressed**. Each sentinel file stores the plan's `fingerprint` (a hash of plan metadata and sources):

```
# .wright-stage-configure
fingerprint=sha256:abc123...
```

Before skipping a Stage, the Pipeline verifies that the stored fingerprint matches the current plan. If the plan has changed, the checkpoint is automatically ignored. This prevents stale checkpoints from silently reusing outdated build artifacts.

---

## State Invalidation Matrix

| User Intent | Affected Layer | CLI Flag | Scope |
|------------|---------------|----------|-------|
| "Rebuild even if staging exists" | L3 Pipeline | `--force` | This plan's entire Pipeline |
| "Rerun from a specific stage" | L3 Pipeline | `--stage=configure` | From that stage forward |
| "Clean everything and start over" | L3 + disk | `--clean` + `rm -rf work/` | Complete reset |

---

## Execution Flow Example

When a user runs `wright apply`, the following flow occurs:

```mermaid
sequenceDiagram
    participant CLI as L1: Operation (apply)
    participant Batch as L2: Batch Driver
    participant PL as L3: Pipeline (Lifecycle)
    participant CK as Checkpoint
    participant ST as L4: Stage (compile)

    CLI->>Batch: Drive batches
    Batch->>PL: Run plan build (with plan fingerprint)
    activate PL
    PL->>CK: is_complete("configure", fingerprint)
    CK-->>PL: true (skip)
    PL->>CK: is_complete("compile", fingerprint)
    CK-->>PL: false (stale — plan changed)
    PL->>ST: Execute Stage
    activate ST
    ST-->>PL: Exit Code 0
    deactivate ST
    PL->>CK: mark_complete("compile", fingerprint)
    PL-->>Batch: Plan Success
    deactivate PL
    Batch->>CLI: Operation Complete
```

---

## Why this Distinction Matters

1.  **Performance:** Managing 800 individual Stages in a global DAG would cause significant scheduling overhead. Grouping them into Pipelines inside batch items keeps the scheduling layer lean.
2.  **Context Preservation:** Stages within a Pipeline often share heavy resources (like a mounted OverlayFS). It is more efficient to run them sequentially within one plan than to teardown/setup environments between Stages.
3.  **Resilience:** The batch driver handles "Process Crashes" (resuming at the plan level), while Pipeline handles "Logical Failures" (resuming at the Stage level using internal checkpoints).
4.  **Safety:** Content-addressed Checkpoints prevent the class of bugs where a plan is modified but old build artifacts are silently reused. The fingerprint mismatch guarantees a clean rebuild.

---

## Design Status & Future Improvements

The current execution hierarchy is **architecturally sound** and satisfies the core requirements of a distro build system. However, several enhancements would move it from "good" to "best-in-class":

### Already Excellent
- **Clean layer separation:** Batch driver does not leak into Pipeline internals.
- **Orthogonal recovery:** Batch execution and Stage Checkpoints are independent caches with clear invalidation semantics.
- **Content-addressed checkpoints:** Eliminates an entire class of stale-build bugs.
- **Deterministic scheduling:** Topological batches ensure reproducible execution order.

### Areas for Improvement

1. **Observability Gap:** There is no `wright status` or `wright log` command to inspect batch progress or stage checkpoints without reading filesystem state directly.
2. **Single-Plan Fast Path:** A `wright build single-plan` still constructs a full execution plan and batch driver. For single-target operations, a direct materialization path would reduce overhead.
3. **Checkpoint Distribution:** Stage Checkpoints are local filesystem files. Distributed builds (e.g. `sccache`-like remote workers) would require checkpoint persistence in shared storage.
4. **`--force` Semantic Overload:** `--force` means "rebuild from scratch" in `wright build` but "reinstall even if installed" in `wright apply`. Consider splitting into `--rebuild` and `--reinstall` for clarity.
5. **Stage-Level Force:** There is no way to force a single stage rerun (e.g. "re-run `check` but keep `compile`"). A `--force-stage=check` flag would be useful.
