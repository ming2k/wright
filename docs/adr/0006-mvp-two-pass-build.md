# ADR-0006: MVP Two-Pass Build for Dependency Cycles

## Status

Accepted

## Context

Some parts have genuine circular build-time dependencies (e.g., `freetype` ↔ `harfbuzz`). These cannot be broken by fixing dependency types because they are real.

Options:

1. Fail the build and require the user to merge the plans.
2. Detect cycles automatically and break them with a two-pass build.

## Decision

Use a two-pass build:

1. **MVP pass**: Build the part with a reduced dependency set (no cyclic dep).
2. **Full pass**: After the rest of the cycle is built, rebuild the part with all dependencies.

Wright uses Tarjan's SCC algorithm to detect cycles. If a plan in the cycle has `[mvp.dependencies]` that remove at least one edge, the two-pass schedule is inserted automatically.

## Consequences

- Circular dependencies are resolved automatically without manual intervention.
- Plans must declare MVP-specific dependencies when they participate in cycles.
- If no MVP definition exists, the build fails with a clear error identifying the cycle.
- The `--mvp` flag allows testing the MVP configuration before a real cycle occurs.
