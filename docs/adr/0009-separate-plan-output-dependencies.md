# ADR-0009: Separate Plan-Level and Output-Level Dependencies

## Status

Accepted

## Context

Currently, `PlanManifest.dependencies` contains four kinds of dependencies:

- `build`: tools needed to compile the plan
- `link`: ABI-sensitive libraries that trigger reverse rebuilds
- `runtime`: libraries/tools needed at install time
- `optional`: optional runtime dependencies

All four are declared at the plan level (`[dependencies]` in `plan.toml`). This creates several problems:

1. **Granularity mismatch**: a plan that produces multiple outputs (e.g. `gcc` → `gcc` + `libstdc++`) may have different runtime requirements per output. `libstdc++` needs `libgcc` at runtime, but the `gcc` compiler binary does not.
2. **Redundancy**: `link` and `runtime` often list the same libraries. Users must declare them twice.
3. **Semantic confusion**: `build` and `link` affect the build planner; `runtime` affects the installer. Mixing them in one table makes it unclear which tool consumes which field.

## Decision

We will separate dependencies into two levels:

### Plan level: build and link

Declared in the top-level `[dependencies]` section. These drive the build orchestrator and dependency resolver.

```toml
[dependencies]
build = ["gcc", "make"]
link = ["freetype", "cairo"]
```

### Output level: runtime and optional

Declared per-output inside `[[output]]` entries. These are recorded in the binary part metadata and enforced at install time.

```toml
[[output]]
name = "libstdc++"
runtime_deps = ["libgcc"]
include = ["/usr/lib/libstdc.*"]
```

If an output does not declare `runtime_deps`, the system falls back to the plan-level `dependencies.runtime` for backward compatibility.

### Database schema

- `plans` table: stores plan-level metadata including `build_deps` and `link_deps`
- `parts` table: links to `plans.id` via `plan_id` foreign key
- `dependencies` table: stores only runtime dependencies (simplified, no `dep_type` column needed since only runtime is persisted)

## Consequences

### Positive

- **Correct granularity**: each output declares only the runtime deps it actually needs.
- **Clearer semantics**: build planner reads plan-level deps; installer reads output-level deps.
- **Plan tracking**: the `plans` table enables plan-scoped operations (list/remove by plan).
- **Reduced duplication**: `link` deps no longer need to be duplicated in `runtime` for dynamic libraries.

### Negative

- **Breaking change for plan authors**: existing `plan.toml` files that declare `runtime` at the plan level for multi-output plans should migrate to `runtime_deps` per output.
- **Migration complexity**: existing installed databases need migration 004 to add the `plans` table and `plan_id` column.

## Migration Path

1. **Backward compatibility**: plan-level `dependencies.runtime` is still supported as a fallback.
2. **Gradual adoption**: authors can move `runtime` from plan level to output level one plan at a time.
3. **Future removal**: in v4.0, plan-level `runtime` may be deprecated and removed.

## Related

- `docs/how-to/bootstrap-new-system.md` — shows `wright assume` for external deps
- `docs/how-to/perform-security-updates.md` — toolchain rebuild order
