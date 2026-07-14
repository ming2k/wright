# ADR-0025: Incremental Cargo Workspace

## Status

Accepted

## Context

Wright has one user-facing executable and a core library containing the CLI,
application workflows, dependency resolution, build execution, archive
handling, and installed-system state. These responsibilities already have
named module boundaries, but the single package does not enforce them.

Several dependencies also crossed the intended layer direction. Application
operations accepted clap argument types, dependency-cycle planning lived in
the build executor while depending on resolver types, and plan validation
depended on build and isolation implementations. Turning the existing module
directories directly into crates would preserve those cycles or require a
large shared crate.

The project needs compile-time boundary enforcement without fragmenting the
installed command or committing every current module boundary to a public
crate API.

## Decision

Wright is a Cargo workspace with one user-facing `wright` package and small
internal library crates extracted incrementally.

An internal crate is extracted only when all of the following hold:

- Its responsibility is stable and independently testable.
- Its dependencies form a one-way edge in the workspace graph.
- Its public surface can be smaller than the implementation it replaces.
- Existing `wright` library paths can be preserved when they are already in
  use.

The first extracted crate is `wright-model`. It owns dependency-free domain
primitives shared by parsing and execution: version and dependency syntax,
the isolation policy value, and the default pipeline stage order. It must not
contain filesystem, database, network, CLI, or process-execution code.

The root package remains the compatibility facade and the only binary. CLI
types are translated into application request types at the CLI boundary.
Dependency graph and bootstrap-cycle planning belong to resolution, while
the foundry executes an already-resolved build plan.

Future crates may be extracted for plan parsing, part archives, persisted
state, and the execution engine after their error and data-transfer
boundaries are explicit. The workspace must remain acyclic. A generic
`common` crate is not introduced; shared code moves to the lowest stable
domain that owns it.

## Alternatives

### Keep one package

Module visibility can improve the current design, but it cannot provide
package-level dependency enforcement or targeted compilation and testing.
The project is large enough for one stable internal boundary to provide
value.

### Split every top-level module into a crate

The current modules do not all form independent components. A mechanical
split would expose implementation details and reproduce circular
dependencies across package boundaries.

### Create separate user-facing binaries

Multiple binaries would not improve the internal dependency graph and would
conflict with the unified CLI established by ADR-0018.

## Consequences

- Cargo enforces the first domain boundary and can test it independently.
- Plan validation and execution share domain semantics without depending on
  each other's implementations.
- The installed command, CLI syntax, and existing root-library paths remain
  compatible.
- Workspace metadata and lint policy are centralized at the repository root.
- Each future extraction requires an explicit error boundary and migration
  of tests; crate count grows only when that cost has a concrete payoff.
- Full workspace builds still compile the application and its heavy system
  dependencies. The first extraction improves architecture more than clean
  build time.
