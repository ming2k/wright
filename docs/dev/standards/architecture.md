# System Architecture Governance

This document codifies the workspace topology, crate ownership, and non-negotiable
architectural red-lines of Wright. The living, crate-by-crate description belongs to
[Module Layout](../module-layout.md); this document states the boundaries that module layout
may not cross.

---

## 1. Layered Workspace Topology

Wright is one public binary backed by a layered Cargo workspace. Dependencies flow inward:
a higher layer may depend on lower layers, never the reverse.

```text
┌──────────────────────────────────────────────────────────────────────┐
│ Layer 5: CLI                                                         │
│   `wright` (root package: `src/bin/wright.rs`, `src/cli/`)           │
│   clap schemas, dispatch, terminal rendering                          │
├──────────────────────────────────────────────────────────────────────┤
│ Layer 4: Application actions                                         │
│   `wright-actions`  Foundry, Seal, Deploy use cases                  │
├──────────────────────────────────────────────────────────────────────┤
│ Layer 3: Planning and scheduling                                     │
│   `wright-resolve`    target resolution, Package DAG, build waves    │
│   `wright-scheduler`  Action DAG, planner, concurrent executor       │
│   (within this layer `wright-resolve` depends on `wright-scheduler`) │
├──────────────────────────────────────────────────────────────────────┤
│ Layer 2: Infrastructure and storage                                  │
│   `wright-registry`   installed index and delivery transactions      │
│   `wright-cache`      closure-fingerprint build artefact cache       │
│   `wright-ledger`     append-only build records and plan snapshots   │
│   `wright-lock`       advisory cross-process file locks              │
│   `wright-sandbox`    namespaces, OverlayFS, resource limits         │
│   `wright-config`     configuration schemas and loaders              │
│   `wright-part`       archives, folios, stores, FHS/ELF validation   │
│   `wright-plan`       manifest parsing, linting, and discovery       │
├──────────────────────────────────────────────────────────────────────┤
│ Layer 1: Shared values                                               │
│   `wright-model`      versions, pipeline stages, isolation policy    │
└──────────────────────────────────────────────────────────────────────┘
```

The actual internal edges are:

| Crate | May depend on |
| :--- | :--- |
| `wright` (root) | every workspace crate except `wright-model` |
| `wright-actions` | `wright-{cache,config,ledger,lock,model,part,plan,registry,resolve,sandbox,scheduler}` |
| `wright-resolve` | `wright-{config,ledger,model,part,plan,registry,scheduler}` |
| `wright-scheduler` | `wright-{cache,ledger,model,part,plan,registry,sandbox}` |
| `wright-registry` | `wright-{ledger,lock}` |
| `wright-part`, `wright-plan`, `wright-config`, `wright-sandbox` | `wright-model` |
| `wright-cache`, `wright-ledger`, `wright-lock`, `wright-model` | nothing internal |

Cargo rejects cycles, but it permits an edge that inverts a layer. Adding such an edge is an
architectural change: it requires an ADR and an update to this table. Two edges stay inside
their layer: `wright-resolve` depends on `wright-scheduler`, and `wright-registry` depends on
`wright-ledger` and `wright-lock`.

---

## 2. Architectural Red-Lines

### 2.1 Single Public Binary

- **Rule**: `wright` is the only executable the workspace produces.
- **Mechanism**: the root `Cargo.toml` declares exactly one `[[bin]]`. New capabilities are
  subcommands of that binary, not sibling executables.

### 2.2 `wright-model` Is Dependency-Free

- **Rule**: `wright-model` contains no filesystem, process, database, network, or
  serialization code, and no internal or external dependencies.
- **Mechanism**: its `Cargo.toml` has no `[dependencies]` table. A dependency added there
  changes the layer contract and must be justified in an ADR.

### 2.3 CLI Types Stop at the CLI Boundary

- **Rule**: clap `Args` types, command dispatch, and CLI-only enums live in the root
  `src/cli/`. `wright-actions` and the other library crates accept request types, never clap
  types.
- **Mechanism**: library crates do not depend on the root package, so they cannot name a CLI
  type. The `run` adapter in `src/cli/<command>.rs` translates arguments into a request
  before entering `wright-actions`.

### 2.4 Fail-Closed Isolation

- **Rule**: when the kernel or the environment refuses to create the requested sandbox,
  the build fails. Wright never silently degrades a `strict` isolation request into host
  execution.
- **Mechanism**: isolation is a typed policy in `wright-model` and executed by
  `wright-sandbox`; the direct runner is selected explicitly, not as a fallback. See
  [ADR-0027](../../adr/0027-isolation-fails-closed.md).

### 2.5 Archives Are Inventory, Not Cache

- **Rule**: sealed `.wright.tar.zst` archives in `parts_dir` are installable inventory and
  the rollback copy. They are not a build-reuse cache and are never deleted by a cache purge.
- **Mechanism**: reuse is decided by the charge marker (`SourceDigest`) and the CAS, not by
  the presence of an archive. See [ADR-0043](../../adr/0043-registry-as-derived-index-and-maintenance-surface.md),
  [ADR-0054](../../adr/0054-content-addressed-source-identity.md), and
  [ADR-0057](../../adr/0057-transactional-cold-purge-and-trust-boundary.md).

### 2.6 One Derivation of Source Content

- **Rule**: every cache key and the audit provenance derive from the same source-hashing
  code. A second implementation of "what do this plan's sources hash to" must not exist.
- **Mechanism**: `wright-actions`'s charge identity produces both the `SourceDigest` used by
  the marker, build key, and checkpoint chain, and the provenance lines written to
  `.PARTINFO`. See [`[INV-SRC-01]`](invariants.md) and
  [ADR-0055](../../adr/0055-plan-drift-covers-bundled-sources.md).

---

## 3. Crate Addition and Workspace Integrity

1. **Crate inclusion threshold**: add a crate only when the responsibility cannot be modeled
   as a module of an existing domain crate, and its position in the layers above is
   unambiguous.
2. **Every internal crate lives under `crates/`.** The root package holds only the binary and
   the CLI adapter; it is not a home for library code.
3. **No passthrough crates**: a crate must not exist solely to re-export another. The
   compatibility re-exports in `wright-actions` (`config`, `resolve`, `isolation`) exist for
   historical import paths; new code imports the owning crate directly.
4. **One direction per boundary**: a lower crate must not depend upward to reuse a helper.
   Move the helper down, or accept the duplication.
