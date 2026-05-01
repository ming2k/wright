# ADR-0002: Wave-by-Wave Install Instead of One Big Install

## Status

Accepted

## Context

`wright apply` must install built parts onto the live system. Two models were considered:

1. **One big install**: Build everything first, then install all archives in a single transaction.
2. **Wave-by-wave install**: Build and install each dependency batch before moving to the next.

## Decision

Use wave-by-wave installation. For every batch:

1. Build the batch.
2. Discover archive outputs.
3. Deduplicate archive paths.
4. Install that batch onto the live system.

## Consequences

- Later waves observe the system as updated by earlier waves. This is essential for self-hosting and rolling maintenance where newly built toolchain or library parts must be visible before building their dependents.
- Dependency order remains explicit and inspectable.
- The live system is updated progressively rather than only at the very end.
- Each install batch succeeds or fails in sequence. `apply` is not a single global transaction across all waves.
- This tradeoff is intentional: correctness of staged system convergence is more important than pretending the entire multi-wave operation is atomic.
