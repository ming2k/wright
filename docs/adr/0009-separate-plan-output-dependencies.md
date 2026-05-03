# ADR-0009: Separate Plan-Level and Output-Level Dependencies

## Status

Accepted

## Context

Previously, `PlanManifest` used a single nested `[dependencies]` table containing three kinds of dependencies:

- `build`: tools needed to compile the plan
- `link`: ABI-sensitive libraries that trigger reverse rebuilds
- `runtime`: libraries/tools needed at install time

All three were declared at the plan level. This created several problems:

1. **Granularity mismatch**: a plan that produces multiple outputs (e.g. `gcc` → `gcc` + `libstdc++`) may have different runtime requirements per output. `libstdc++` needs `libgcc` at runtime, but the `gcc` compiler binary does not.
2. **Semantic confusion**: `build` and `link` affect the build planner; `runtime` affects the installer. Mixing them in one table makes it unclear which tool consumes which field.
3. **Parsing complexity**: the nested `[dependencies]` section required extra deserialization logic and was error-prone.
4. **Optional ambiguity**: "optional dependencies" are not dependencies at all — they are features. Declaring them alongside real dependencies creates confusion about what the installer should enforce.

## Decision

We will flatten and separate dependencies into two distinct levels with no overlap:

### Plan level: `build` and `link`

Declared as **top-level fields** in `plan.toml`. These drive the build orchestrator and dependency resolver. They are never serialized into binary parts.

```toml
name = "gcc"
version = "14.2.0"
release = 1
# ...

build = ["gcc", "make"]
link = ["freetype", "cairo"]
```

### Output level: `runtime_deps`

Declared **per-output** inside `[[output]]` entries. These are recorded in binary part metadata (`.PARTINFO`) and enforced at install time. There is **no plan-level fallback** — if an output needs runtime deps, it must declare them explicitly.

```toml
[[output]]
name = "libstdc++"
runtime_deps = ["libgcc"]
include = ["/usr/lib/libstdc.*"]
```

### No `[dependencies]` section, no optional dependencies

The `[dependencies]` table is **removed entirely** from `plan.toml`. `RawManifest` uses `#[serde(deny_unknown_fields)]`, so any plan-level `[dependencies]` section is rejected at parse time. This is a clean break — no backward compatibility for the old syntax.

**Optional dependencies are removed entirely.** If a feature requires an external library, declare it as a normal `runtime_deps` and document it in the part description. The user decides whether to install it. Wright does not track "optional" dependencies because the concept is ambiguous — optional for whom? Under what conditions?

### Database schema

- `plans` table: stores plan-level metadata including `build_deps` and `link_deps` as JSON arrays
- `parts` table: links to `plans.id` via `plan_id` foreign key
- `dependencies` table: stores only runtime dependencies (simplified `DepType` enum with only `Runtime`)
- ~~`optional_dependencies` table: removed~~

### Single-output plans

Even single-output plans use `[[output]]` to declare runtime dependencies:

```toml
name = "nginx"
# ...

build = ["perl", "gcc", "make"]
link = ["openssl", "zlib"]

[[output]]
name = "nginx"
runtime_deps = ["openssl", "zlib"]
```

## Consequences

### Positive

- **Correct granularity**: each output declares only the runtime deps it actually needs.
- **Clearer semantics**: build planner reads plan-level `build`/`link`; installer reads output-level `runtime_deps`.
- **Simpler TOML**: flat top-level fields are easier to read and write than nested tables.
- **No optional ambiguity**: every dependency is either required at build time (`build`/`link`) or required at runtime (`runtime_deps`). There is no "maybe" category.
- **Plan tracking**: the `plans` table enables plan-scoped operations (list/remove by plan).

### Negative

- **Breaking change**: all existing `plan.toml` files using `[dependencies]` must migrate.
- **Verbosity for single-output plans**: runtime deps must now be wrapped in `[[output]]` even when there is only one output.

## Migration Path

1. Replace `[dependencies] build = [...]` with top-level `build = [...]`
2. Replace `[dependencies] link = [...]` with top-level `link = [...]`
3. Move `[dependencies] runtime = [...]` into each `[[output]]` as `runtime_deps = [...]`
4. Remove `[dependencies] optional = [...]` entirely — document features in `description` instead
5. Remove the `[dependencies]` section entirely

## Related

- `docs/how-to/bootstrap-new-system.md` — shows `wright assume` for external deps
- `docs/how-to/perform-security-updates.md` — toolchain rebuild order