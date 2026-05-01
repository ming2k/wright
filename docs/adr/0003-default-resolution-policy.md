# ADR-0003: Default Resolution Policy for Apply

## Status

Accepted

## Context

`wright apply` needs a sensible default for dependency expansion. Options included:

1. No expansion (build only explicit targets).
2. Expand all dependencies unconditionally.
3. Expand only missing and outdated dependencies.
4. Expand reverse dependents too.

## Decision

Default to `--deps --match=outdated`:

- Explicit targets are included when missing or differ from installed plan state.
- Dependencies are expanded across build, link, and runtime edges.
- Missing and outdated dependencies are auto-added.
- Reverse dependent expansion is disabled by default.
- Depth is unlimited for dependency traversal.

## Consequences

- Adding missing and outdated dependency plans is the minimum useful "smart" behavior for a source-first convergence command.
- `apply` does **not** default to reverse rebuild cascades. Rebuilding dependent dependents is a heavier policy decision and remains an explicit low-level workflow through `wright resolve --rdeps`.
- Users who want different behavior can drop to `wright resolve` explicitly.
