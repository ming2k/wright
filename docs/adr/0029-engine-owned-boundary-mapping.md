# ADR-0029: Engine-Owned Workspace Boundary Mapping

## Status

Accepted

## Context

ADR-0026 established six workspace packages and a one-way dependency graph.
The initial extraction encoded two convenient but misleading edges:
`wright-state` depended on `wright-part` to accept parsed archive metadata,
and `wright-part` depended on `wright-plan` to seal a manifest directly.

Those edges made persistence aware of an archive representation and made the
archive writer aware of the complete source-manifest representation. A change
to plan parsing could therefore rebuild the archive and persistence crates,
even though neither responsibility had changed. Compatibility re-exports in
`wright-engine` also obscured which crate owned an imported type.

The workspace needs to preserve its existing responsibilities while making
cross-boundary data conversion explicit at the application orchestration
layer.

## Decision

The six packages established by ADR-0026 remain. Their dependency direction
is refined to:

```text
wright -> wright-engine -> wright-plan  -> wright-model
                         -> wright-part  -> wright-model
                         -> wright-state
```

The root `wright` package may depend on lower crates solely to preserve
established compatibility paths. Runtime orchestration continues to enter
through `wright-engine`.

`wright-plan`, `wright-part`, and `wright-state` are sibling boundaries:

- `wright-part` owns the archive input and output types. The engine projects a
  plan manifest into the archive input before sealing.
- `wright-state` owns persistence input types. The engine projects parsed part
  metadata into a persistence input before registration.
- Lower crates do not depend on a sibling crate to accept its representation.
  Shared dependency-free semantics remain in `wright-model`.
- Internal engine code imports the crate that owns a type. Compatibility
  re-exports live in the root package rather than in `wright-engine`.
- Higher layers wrap lower-layer errors transparently instead of copying each
  lower error variant into their own error enum.

No generic transfer-object crate or dependency-injection framework is added.
Boundary inputs remain small, responsibility-specific types owned by the
receiving crate.

## Alternatives

### Keep the layered dependency chain

The graph was acyclic, but its edges represented data-transfer convenience
rather than responsibility. It also increased rebuild scope for lower-level
changes.

### Move all transfer types into `wright-model`

Archive and persistence inputs describe I/O formats rather than shared,
dependency-free domain semantics. Moving them into `wright-model` would turn
that crate into a generic shared-types package.

### Merge plan, part, and state

Merging would remove conversion code but also discard independently testable
parser, archive, and persistence boundaries and combine their unrelated heavy
dependencies.

### Add more subsystem crates

Foundry, resolution, isolation, and transactions still collaborate as one
application engine. Further extraction would add public transfer types without
removing a false lower-layer dependency.

## Consequences

- Plan, archive, and persistence changes have a smaller compilation and review
  blast radius.
- Cargo continues to enforce the CLI-to-engine boundary and the
  dependency-free model boundary.
- The engine contains explicit, testable mapping code between sibling
  components.
- Compatibility paths remain available from the root package without becoming
  the engine's internal import convention.
- Boundary input types duplicate a small projection of source data. That
  duplication is intentional: each receiving crate controls its own contract.
- Changes to archive or persistence schemas require updating the corresponding
  engine mapping and boundary tests.
