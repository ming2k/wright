# ADR-0026: Workspace Crate Boundaries

## Status

Superseded by ADR-0029

## Context

ADR-0025 introduced an incremental Cargo workspace and extracted the first
dependency-free domain crate. The remaining application code still compiled
inside the root package, so module visibility rather than Cargo enforced the
boundaries between plan parsing, part archives, persistence, orchestration,
and the CLI.

The dependency cycles that blocked a full split have been removed. CLI
arguments translate into application requests, bootstrap-cycle planning
belongs to resolution, plan metadata expansion no longer depends on the
foundry, and database callers no longer access the SQL pool directly.

## Decision

The workspace has six packages with the following dependency direction:

```text
wright -> wright-engine
              |-> wright-state -> wright-part -> wright-plan -> wright-model
              |-> wright-part  -> wright-plan -> wright-model
              |-> wright-plan  -> wright-model
              +-> wright-model
```

- `wright` owns the clap command tree, dispatch adapters, the binary entry
  point, and compatibility re-exports. It remains the only installed binary.
- `wright-engine` owns configuration, use cases, dependency resolution,
  foundry execution, isolation, sealing, transactions, queries, and
  application-facing diagnostics.
- `wright-state` owns the installed SQLite state, immutable migrations,
  delivery recovery, content-addressed storage, and process locks.
- `wright-part` owns part and folio formats, archive compression, local part
  stores, FHS validation, and ELF/SONAME inspection.
- `wright-plan` owns plan parsing, validation, discovery, and plan metadata
  expansion.
- `wright-model` owns dependency-free shared values: version and dependency
  syntax, isolation policy values, and default pipeline stages.

Every library crate owns its error type. Higher layers convert lower-layer
errors at their boundary. No lower crate depends on `wright-engine` or
`wright`, and no library crate depends on clap.

The root library re-exports established module paths from `wright-engine` so
existing integration code can continue to use paths such as `wright::plan`
and `wright::database`. New internal code imports the owning crate directly.

## Alternatives

### Leave orchestration in the root package

This would keep the binary package coupled to every system dependency and
would leave the most important CLI-to-application boundary unenforced.

### Split each engine subsystem into its own crate

Foundry, resolution, sealing, isolation, and transactions collaborate as one
application engine and still share orchestration types. Splitting each module
would add public transfer types without creating an independent reuse or
test boundary.

### Publish every internal crate

The internal crates are implementation boundaries, not a commitment to a
stable third-party SDK. They remain path dependencies in the workspace.

## Consequences

- Cargo rejects reverse dependencies between the CLI, engine, persistence,
  archive, plan, and model layers.
- Unit tests run in the crate that owns the behavior; root-package tests
  exercise the public compatibility facade and end-to-end workflows.
- The root package has only CLI/runtime and engine dependencies instead of
  every archive, database, network, and isolation dependency.
- Database migrations move with `wright-state`; their immutability rule is
  unchanged.
- Cross-crate APIs and error conversions require deliberate maintenance.
- Moving behavior between crates requires updating package manifests and may
  increase the number of compilation units, but does not change the installed
  command or CLI behavior.
