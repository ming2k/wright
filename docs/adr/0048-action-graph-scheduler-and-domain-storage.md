---
id: ADR-0048
title: "Action Graph Scheduler and Domain-Centric Storage Architecture"
status: accepted
date: 2026-03-30
scope: core/engine
superseded_by: null
negative_knowledge: true
---

# 0048. Action Graph Scheduler and Domain-Centric Storage Architecture

- Status: Accepted
- Date: 2026-03-30
- Deciders: Core Engineering Team
- Consulted: Architecture Group
- Informed: Maintainers

---

## Context and Problem Statement

Wright's execution pipeline and crate boundaries were established through incremental architectural evolution ([ADR-0002](0002-wave-by-wave-install.md), [ADR-0018](0018-unified-cli-porcelain-plumbing.md), [ADR-0026](0026-workspace-crate-boundaries.md), [ADR-0029](0029-engine-owned-boundary-mapping.md), and [ADR-0043](0043-registry-as-derived-index-and-maintenance-surface.md)). Over time, several architectural tensions have emerged at the boundary between orchestration, concurrency, and persistence:

1. **Wave Barrier Synchronization Bottleneck (Long-Tail Critical Path)**:
   In [ADR-0002](0002-wave-by-wave-install.md), Wright adopted a wave-by-wave install loop: resolve the package graph into topological batches, build the batch in parallel, seal parts, and deploy them together onto the live system before proceeding to the next wave. While this satisfied the core correctness requirement that dependent packages in subsequent waves observe newly installed binaries and libraries, it introduced an artificial **Batch Barrier**. A lengthy compile task in Wave $N$ (e.g. `gcc` or `llvm`) blocks packages in Wave $N+1$ from starting their build, even when those downstream packages only depend on small libraries in Wave $N$ (e.g. `zlib`) that finished deploying minutes earlier.

2. **Imperative Monolithic Orchestration**:
   High-level workflow commands (such as `execute_install` and `execute_upgrade`) implemented over 1,000 lines of procedural coordination: hardcoding signal cancellation loops, build semaphores, CAS cache lookups, ABI diff probes, and SQLite delivery transactions. Adding new operational intents or extending verification required duplicating complex procedural control flow across operations.

3. **God-Crate Boundary (`wright-engine`)**:
   `wright-engine` currently bundles disparate system layers: low-level OS namespace and OverlayFS sandboxing (`isolation`), declarative dependency graph resolution (`resolve`), stage compilation pipelines (`foundry`), part slicing and packaging (`seal`), atomic filesystem rollback journals (`transaction`), and CLI workflow command handlers (`operations/*`). This prevents fine-grained unit testing of resolution logic without dragging in sandbox dependencies and harms incremental compilation.

4. **Persistence Overloading and Semantic Leakage (`wright-state`)**:
   `crates/wright-state` evolved into a catch-all persistence crate: housing the SQLite relational database (`InstalledDb`), local content-addressed storage on disk (`CasStore`), file-backed build audit ledgers (`ledger.rs`), delivery transaction recovery (`delivery.rs`), and cross-process file locks (`lock.rs`). Domain layers (such as `resolve`) that only need to query installed package metadata are forced to pull in the entire SQLite engine, migrations, and file-locking substrates.

5. **Implementation Jargon vs. Domain Intent**:
   Naming the package reuse cache `CasStore` exposed a low-level computer science storage mechanism (Content-Addressed Storage) rather than domain intent. In package management, the operator's business intent is a **Build Cache** for zero-latency artifact reuse.

A durable, uncompromised, long-term oriented architecture is required to unify execution into a declarative task graph while cleanly decomposing domain storage boundaries without legacy baggage.

---

## Decision Drivers

- **Maximum Pipeline Concurrency**: Eliminate the coarse wave barrier. Enable fine-grained, point-to-point pipelining where independent tasks begin building as soon as their specific prerequisites deploy.
- **Dual-DAG Separation of Concerns**: Strictly separate high-level domain resolution (Package DAG) from physical execution scheduling (Action DAG).
- **Physical "Three Trees" Model**: Formulate all state transitions as transformations across three directory trees: Source Tree, Staging Tree, and Live Root Tree.
- **Standardized Ubiquitous Atom Vocabulary**: Formulate a closed set of 9 orthogonal action verbs covering compilation, verification, cache bypass, delivery, and Saga compensation.
- **Domain-Driven Storage Separation**: Decompose `wright-state` into decoupled, single-responsibility crates (`registry`, `cache`, `ledger`), isolating process synchronization.
- **Fail-Closed Verification & Dynamic Pruning**: Retain runtime feedback capabilities: zero-latency build cache restoration and ADR-0045 deterministic ABI probe inhibition.

---

## Considered Options

- **Option 1: Dual-DAG Architecture with Domain-Centric Storage Subsystems (Chosen)**
  - Retain the **Package DAG** for logical dependency resolution, SAT solving, human-facing CLI progress, and high-level transaction accounting.
  - Compile the Package DAG into a fine-grained **Action DAG** where nodes are individual, typed action verbs (`Lint`, `Fetch`, `Build`, `RestoreCache`, `Seal`, `VerifyAbi`, `Deploy`, `CommitRegistry`, `Rollback`).
  - Connect cross-package dependencies directly between physical actions (`Deploy(A) -> Build(B)`), turning the Wave model into fine-grained pipelining.
  - Decompose `wright-state` into `wright-registry` (SQLite installed index), `wright-cache` (build artifact cache), and `wright-ledger` (append-only build records and snapshots).
- **Option 2: Status Quo (Procedural Wave Loop with God Crate)**
  - Maintain the procedural batch loops in `execute_install` and keep `wright-engine` and `wright-state` as combined monolithic packages. (Discarded: severe long-tail concurrency bottlenecks and persistent architectural debt).
- **Option 3: Monolithic Mega-Graph (Merging Package and Action into One Graph)**
  - Merge package metadata and execution actions into a single heterogeneous graph structure. (Discarded: leads to type ambiguity, pollutes solver algorithms with runtime execution state, and entangles resolution with execution).
- **Option 4: Extreme Fine-Grained Action DAG (Bazel-Style Statement Actions)**
  - Decompose actions down to individual shell commands, compiler invocations (`gcc -c foo.c`), and directory creations. (Discarded: excessive graph overhead, memory explosion, and high runtime scheduling latency unnecessary for package-level orchestration).

---

## Decision Outcome

Chosen option: **Option 1: Dual-DAG Architecture with Domain-Centric Storage Subsystems**.

### 1. Dual-DAG Co-existence Model

The architecture separates declarative intent from physical execution into two co-existing graph representations:

```text
 ┌───────────────────────────────────────────────────────────────┐
 │  1. Package DAG (Logical Dependency Graph)                    │
 │     • Node: PackageId { name, version, release, epoch }       │
 │     • Edges: build, link, and runtime dependency declarations  │
 │     • Output of: wright-resolve                               │
 │     • Scope: Version convergence, human progress, accounting │
 └───────────────────────────────┬───────────────────────────────┘
                                 │
                                 │ Lowered by Planner
                                 ▼
 ┌───────────────────────────────────────────────────────────────┐
 │  2. Action DAG (Physical Execution Graph)                     │
 │     • Node: ActionNode { id, package_id, action, status }     │
 │     • Edges: Physical execution constraints & causal orders   │
 │     • Managed by: wright-scheduler (Task Scheduler)           │
 │     • Scope: Worker dispatch, resource locks, dynamic pruning │
 └───────────────────────────────────────────────────────────────┘
```

The graphs co-exist throughout command execution:
- The **Package DAG** is produced by resolution, immutable, and retained as the structural reference.
- The **Action DAG** is generated by the Planner. Each `ActionNode` carries a `package_id` lineage tag linking back to its originating package.
- When all actions associated with a `package_id` complete successfully, the scheduler marks the package as complete in the Package DAG, providing accurate, stable CLI output.

---

### 2. The Three Trees Physical Model

All package manager operations are defined as deterministic transformations across three concrete directory trees:

```text
 ┌─────────────────────┐       ┌──────────────────────┐       ┌─────────────────────┐
 │  Tree 1: Source     │       │  Tree 2: Staging     │       │  Tree 3: Live Root  │
 │  (Declared Sources) │ =====>│  (Isolated Sandbox)  │ =====>│  (Target Rootfs)    │
 │                     │       │                      │       │                     │
 │ • plans/<pkg>/      │       │ • /build (workspace) │       │ • /usr/bin          │
 │ • source/ (sources) │       │ • /staging (FHS tree)│       │ • /usr/lib          │
 └─────────────────────┘       └──────────────────────┘       └─────────────────────┘
```

1. **Tree 1: Source Tree**: Immutable declarations (`plan.toml`) and verified upstream source tarballs/git repositories.
2. **Tree 2: Staging Tree**: Ephemeral, unprivileged sandbox directories (`/build` and `/staging`), completely isolated via Linux namespaces and OverlayFS.
3. **Tree 3: Live Root Tree**: The destination host root (`/`) or target root (`--root <path>`). Modifying this tree introduces global side-effects and requires transactional safety.

---

### 3. Canonical Vocabulary of 9 Action Atoms

The Action DAG is composed exclusively of 9 typed, orthogonal action verbs:

| Tree Boundary | Action Atom | Inputs / Preconditions | Output / State Transition | Resource Demands & Side Effects |
| :--- | :--- | :--- | :--- | :--- |
| **Tree 1** | **`Lint`** | `plan.toml` | In-memory validation report | Read-only; CPU trivial |
| **Tree 1** | **`Fetch`** | Remote URL / Git Ref | Populates `source/` with verified checksum | Network IO; zero host side-effects |
| **Tree 1 $\to$ Tree 2** | **`Build`** | `source/` + dependencies | Populates `/staging` via isolated sandbox | Heavy CPU/RAM; sandbox-confined |
| **Tree 1 $\to$ Tree 2** | **`RestoreCache`** | Fingerprint match | Unpacks cached archive directly to parts store | Fast disk IO; bypasses compilation |
| **Tree 2** | **`Seal`** | `/staging` FHS directory | Emits `.wright.tar.zst` part archive | Sequential disk IO; Zstd compression |
| **Tree 2** | **`VerifyAbi`** | Staged ELF `.so` libraries | Decision signal: `Compatible` or `Incompatible` | CPU read-only; triggers dynamic pruning |
| **Tree 2 $\to$ Tree 3** | **`Deploy`** | `.wright.tar.zst` archive | Unpacks payload into Live Root (`/`) | ⚠️ **Global host mutation**; root write lock |
| **Tree 3** | **`CommitRegistry`** | Installed metadata, files | Persists registration in SQLite database | Database transaction write |
| **Tree 3** | **`Rollback` / `Unmerge`** | Transaction ID / Part ID | Removes unmerged files from Live Root | ⚠️ **Global host mutation**; Saga compensation |

---

### 4. Dynamic DAG Pruning and Pipelining

1. **Point-to-Point Pipelining (Wave Boundary Dissolution)**:
   The Planner dissolves the coarse-grained batch barrier. When Package $B$ depends on Package $A$, the Planner links:
   $$\text{Deploy}(A) \xrightarrow{\text{dependency}} \text{Build}(B)$$
   Package $B$ begins compiling the moment Package $A$ finishes deployment, regardless of whether other unrelated packages in $A$'s logical wave are still compiling.

2. **Runtime ABI Probe Inhibition**:
   When `VerifyAbi` determines that a newly compiled package is backward compatible with the currently deployed version, the scheduler intercepts downstream dependent tasks and dynamically marks downstream rebuild actions as `Skipped`, eliminating unnecessary cascading builds (ADR-0045).

3. **Cache-Hit Bypass**:
   Before dispatching a `Build` action, the scheduler evaluates the build closure fingerprint against `wright-cache`. On a hit, `Build` is dynamically substituted with `RestoreCache`.

---

### 5. Domain-Centric Storage Architecture

`crates/wright-state` is dismantled and refactored into focused domain crates:

```text
crates/
├── wright-registry/   # Relational state index (SQLite: installed plans, parts, files, dependencies)
├── wright-cache/      # Build artifact reuse cache (Content-addressed .part archives; formerly CAS)
├── wright-ledger/     # File-backed immutable audit ledger (builds.jsonl, plan snapshots; ADR-0041)
└── wright-common/     # Inter-process synchronization primitives (flock locks)
```

- **`wright-registry`**: Implements the ADR-0043 mandate ("Registry is a derived index"). Exposes read-only trait query interfaces (`trait RegistryQuery`) to decouple resolution from SQLite drivers.
- **`wright-cache`**: Explicitly replaces the low-level `CasStore` moniker with domain-first `BuildCache` APIs.
- **`wright-ledger`**: Manages plain-file audit trails completely detached from transactional state.

---

### 6. Workspace Crate Boundaries (5-Layer Onion Architecture)

The workspace crate boundaries are restructured into five strict, acyclic layers:

```text
Layer 5: CLI Surface
  └── wright (Binary entry point, Clap arguments, terminal UI)

Layer 4: Orchestration & Scheduling
  └── wright-scheduler (Task scheduler, Action DAG, concurrency semaphores, Saga rollback)

Layer 3: Planning & Actions
  ├── wright-resolve (Package DAG resolution, SAT constraint solving)
  └── wright-actions (Implementations of Build/Foundry, Seal, and Deploy)

Layer 2: Domain Infrastructure & Storage
  ├── wright-sandbox (Linux namespaces, chroot, OverlayFS isolation)
  ├── wright-registry (Installed SQLite state index)
  ├── wright-cache (Build artifact reuse store)
  ├── wright-ledger (Audit ledger and plan snapshots)
  ├── wright-part (Archive packaging and ELF/ABI inspection)
  └── wright-plan (Plan parsing, linting, discovery)

Layer 1: Shared Primitives
  └── wright-model (Dependency-free value objects: Version, Stage, Identifiers)
```

*Note on Consolidation*: Action execution providers (`Build`, `Seal`, `Deploy`) reside within `wright-actions` rather than separate micro-crates, preventing crate proliferation and error-wrapping ceremony.

---

## Positive Consequences

- **Dramatically Reduced Latency**: Independent downstream tasks compile concurrently while long-tail upstream packages complete, cutting total critical path time on multi-core workstations.
- **Unified Engine Simplicity**: Subcommands (`install`, `upgrade`, `build`, `remove`) compile their specific intent into an Action DAG and delegate execution to a single robust, observable scheduler.
- **Deterministic Testing**: The Planner is a pure, side-effect-free function: unit tests verify complex Action DAG topologies and edge constraints in milliseconds without root privileges or filesystem mounts.
- **Clean Persistence Boundaries**: Resolvers and inspection commands interact with `wright-registry` through lightweight interfaces without pulling in migration engines or locking subsystems.
- **Ubiquitous Domain Language**: Eliminates low-level jargon (`cas`, `state`) in favor of domain terms (`BuildCache`, `Registry`, `Ledger`).

---

## Negative Consequences

- **Scheduler Complexity**: The async event-driven scheduler requires robust state machine management (`Pending -> Ready -> Running -> Succeeded/Failed/Skipped`) and graceful signal propagation.
- **Saga Compensation Overhead**: Deploy actions that modify the live system prior to full pipeline completion require infallible local rollback handlers (`fs_tx`) in the event of late-stage failures.
- **Migration Effort**: Phased refactoring requires careful bridging to avoid breaking active CLI commands during the transition.

---

## Rejected Alternatives & Negative Knowledge

### Why Coarse Wave Barriers (Status Quo) Were Discarded

The strict wave-by-wave model was originally designed to guarantee that dependent packages observe freshly deployed files before compiling. However, enforcing this via a global synchronous barrier across all packages in a wave creates severe head-of-line blocking. A slow, leaf package in Wave 1 blocks unrelated packages in Wave 2 from proceeding. Point-to-point Action DAG edges achieve the exact same dependency visibility guarantee without blocking unrelated packages.

### Why Monolithic Single-Graph Unification Was Discarded

Attempting to merge packages and actions into a single graph data structure creates a bloated heterogeneous graph. Algorithms that operate exclusively on package semantics (such as dependency satisfiability and reverse dependent expansion) would have to continually filter out action nodes. Keeping Package DAG and Action DAG distinct and linked via `package_id` lineage preserves optimal separation of concerns.

### Why Bazel-Style Micro-Actions Were Discarded

Tools like Bazel represent individual compiler subprocesses (`gcc -c file.c`) as action nodes. In Wright, the primary unit of authoring and distribution is the Plan. Breaking a build into thousands of micro-actions inside Rust would incur severe scheduling overhead, memory amplification, and IPC cost without measurable concurrency gains, as the internal stage pipeline and `make`/`ninja` already parallelize at the file level inside the sandbox.

### Why Retaining `wright-state` as a Unified Crate Was Discarded

Grouping SQLite state, CAS blob storage, ledger files, and process locks under `wright-state` caused widespread semantic leakage. Query-only subcommands incurred SQLite write-lock and recovery overhead, and changing CAS storage mechanics invalidated relational schema boundaries. Physical decomposition into `registry`, `cache`, and `ledger` reflects actual operational lifecycles.
