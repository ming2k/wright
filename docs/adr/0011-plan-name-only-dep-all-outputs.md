# ADR-0011: Plan-Name-Only Dependency References Resolve to All Outputs

## Status

Accepted

## Context

Wright's dependency syntax requires `plan:output` for every dependency reference:

```toml
build_deps = ["openssl:default", "zlib:default"]
```

The `:default` suffix is a magic string that is hard-coded to resolve to the plan's primary output name (`openssl:default` → `openssl:openssl`). This creates several problems:

1. **Misleading naming**: `:default` implies "use whatever the plan considers its default output," but it actually means "use the output whose name matches the plan name." When a plan has multiple outputs, there is no conceptual "default" — each output is explicit.
2. **Unnecessary verbosity**: For single-output plans, the author must repeat the plan name twice (`cmake:cmake`) or use the confusing `:default` alias. Both are boilerplate.
3. **Semantic gap**: A bare plan name like `cmake` currently has no meaning in dependency fields, even though it is the most natural way to say "I depend on this plan."

## Decision

A bare plan name (without `:`) in any dependency field resolves to **all outputs declared by that plan**.

An explicit `plan:output` reference resolves to **exactly one output**.

The `:default` magic string is **deprecated** and will be removed in a future release.

### Syntax

| Form | Meaning | Example |
|------|---------|---------|
| `plan` | All outputs of `plan` | `cmake` → `cmake:cmake`, `cmake:dev`, `cmake:doc` (if declared) |
| `plan:output` | Exactly one output | `llvm:llvm-libs` |

### Behavior per dependency kind

- **`build_deps`**: Every output directory is mounted read-only into the isolation environment. The build script sees merged contents (identical paths overlay; conflicting paths are an error).
- **`link_deps`**: Every output is tracked as an ABI-sensitive dependency. If **any** output of the referenced plan changes, the dependent plan is flagged for rebuild.
- **`runtime_deps`** (per-output inside `[[output]]`): Every output is recorded as a runtime dependency of the declaring output. Installing the declaring output requires all referenced outputs to be present.

### Why not keep `:default`?

`:default` was a stop-gap to avoid typing the plan name twice. With bare-plan-name support, `:default` serves no purpose — it is strictly less expressive than either `plan` (all outputs) or `plan:plan` (primary output). Removing it eliminates a source of confusion.

### Migration path

1. Replace `:default` with the explicit output name (usually the plan name itself):
   ```toml
   # before
   build_deps = ["openssl:default", "zlib:default"]
   # after
   build_deps = ["openssl", "zlib"]
   ```
2. If you previously depended on `:default` because you only wanted the primary output, use the explicit form:
   ```toml
   build_deps = ["cmake:cmake"]
   ```

## Consequences

### Positive

- **Less boilerplate**: Single-output plans no longer need `:default` or duplicated names.
- **Clearer semantics**: `cmake` means "the plan `cmake`"; `cmake:cmake` means "the specific output `cmake` of plan `cmake`."
- **Matches user intuition**: Authors naturally write `cmake` when they mean "I need cmake."
- **Future-proof**: If a plan later splits into multiple outputs, existing `plan` references automatically include the new outputs without editing every dependent plan.

### Negative

- **Breaking change**: `:default` is rejected at parse time once the deprecation period ends.
- **Wider dependency graphs**: `plan` references pull in more outputs than `:default` did. For plans with heavy `-dev` or `-doc` outputs, this could increase build-time mount overhead. Mitigation: prefer explicit `plan:output` when you know you only need one output.
- **More complex linting**: `wright lint` must validate that a referenced plan exists and enumerate its outputs, rather than checking a single output name.

## Implementation notes

### Phase 1 — Parser support

Update `parse_dep_ref` and `parse_dependency_ref` in `src/part/version.rs` to return a list of resolved `(plan, output)` pairs instead of a single pair. A bare plan name triggers a lookup of the plan's manifest to collect output names.

### Phase 2 — Call-site migration

Update every consumer of dependency references:

- `src/builder/orchestrator/planning.rs` — expand bare names before adding to build queue
- `src/builder/lifecycle.rs` — mount all output directories for bare-name build deps
- `src/commands/lint.rs` — validate all outputs exist for bare-name references
- `src/transaction/install.rs` — install all outputs for bare-name runtime deps
- `src/builder/mvp.rs` — same expansion for MVP dependency graph

### Phase 3 — Deprecation

Emit a warning when `:default` is encountered, pointing to this ADR. After two releases, turn the warning into a hard error.

## References

- `src/part/version.rs` — dependency reference parsing
- `docs/adr/0009-separate-plan-output-dependencies.md` — prior dependency syntax ADR
- `docs/reference/plan-manifest.md` — dependency syntax documentation
