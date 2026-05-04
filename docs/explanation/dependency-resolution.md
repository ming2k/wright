# Dependency Resolution (User View)

This document explains how Wright resolves and acts on dependencies from a user perspective. It focuses on what happens when you build and install parts, and how to interpret the output.

**Dependency Types**
Wright uses two dependency types, each with a different purpose.

- `link_deps`: ABI-sensitive dependencies used by `wright resolve` to trigger reverse rebuilds when a linked dependency changes.
- `runtime`: Required for the part to run after installation.

`link_deps` and `runtime` are allowed to overlap. If something is needed after installation, it must be listed in `runtime` even if it also appears in `link_deps`.

**Where Dependencies Come From**
Dependencies are declared in `plan.toml` at two levels:

- **Plan level**: `link_deps` is a top-level field that drives the build orchestrator.
- **Output level**: `runtime_deps` is declared inside each `[[output]]` entry. It describes what a specific installed part needs at run time.

Only `runtime_deps` and part relations are serialized into binary part metadata used by `wright install`. `link_deps` remains a build-graph concept used by `wright resolve`.

You do not need to declare transitive dependencies. Wright expands them when you run builds that require it.

**What `wright resolve` Does With Dependencies**
`wright resolve` is the command that performs dependency-driven expansion.

- It resolves your targets and prints plan names for `wright build`.
- It expands missing dependencies upward.
- It can expand reverse rebuilds downward.

**Upward Expansion: Missing Dependencies**
By default, `wright build` builds only the listed targets. Add `--deps` to
`wright resolve` when you want Wright to expand dependencies from the
hold tree before building.

- With `wright resolve --deps`, dependencies in the selected domain are added to the output target set.
- With `wright resolve --deps --match=missing`, only dependencies that are not currently installed are added.
- With `wright resolve --deps --match=outdated`, dependencies whose installed epoch/version/release differs from `plan.toml` are added, and missing ones are also included.
- If the dependency is missing and no plan exists, the build fails with a clear error.

With `--deps=all`, Wright expands more aggressively:

- `link_deps` and `runtime` dependencies are added to the resolved target set.
- This is useful for deep rebuilds when you want a clean, consistent dependency chain.

**Downward Expansion: Reverse Rebuilds**
When a dependency changes, other parts may need to be rebuilt.

- `link_deps` dependencies always trigger reverse rebuilds via `wright resolve --rdeps`.
- `runtime` dependencies only trigger reverse rebuilds with `--rdeps=all`.

This behavior keeps ABI-sensitive chains correct without forcing expensive rebuilds by default.

This rebuild behavior does not make `link_deps` an implicit runtime dependency. Runtime requirements must still be declared in `runtime`.

**Scheduling Labels**
`wright build` logs a scheduling plan before building. Each entry includes an
action label and its depth in the dependency graph:

- `build`: Normal build for an explicitly requested target or an added dependency.
- `relink`: Rebuilt because a `link_deps` dependency changed.
- `rebuild`: Rebuilt because of `--rdeps=all` transitive expansion (via `wright resolve`).
- `build:mvp`: Bootstrap build used to break a dependency cycle.
- `build:full`: Full build after an MVP bootstrap.

**Dependency Cycles and MVP Builds**
If Wright detects a dependency cycle, it tries to resolve it in a user-friendly way.

- If the part declares `[mvp]` overrides via a sibling `mvp.toml`,
  Wright performs a two-pass build.
- The first pass is an **MVP build** (labeled `build:mvp` in the scheduling log).
 It excludes the dependencies listed in that MVP override.
- The second pass is a full build, forced to rebuild even if a partial archive exists.

This results in two scheduled entries for that part:

- `build:mvp` — first pass with reduced dependencies
- `build:full` — second pass with all dependencies

If no MVP definition exists, Wright stops and reports the cycle.

**Applying Plans to the Live System**
Wright exposes separate build and install flows:

- `wright build` creates archives from plans.
- `wright install` installs archives onto the live system.

For the common source-first workflow, use `wright apply`. It resolves plans or
assemblies, checks the local archive inventory, automatically adds missing or
outdated dependency plans, builds what is needed, and then installs or
upgrades the requested outputs. In other words, `apply` is the natural
plan-driven install/upgrade/dependency combo command.

**Common Examples**
Example: Build only the listed target.

```bash
wright build curl
```

Example: Build and install from plans while automatically materializing missing
or outdated dependencies.

```bash
wright apply curl
```

Example: Force a deep rebuild of dependencies.

```bash
wright resolve openssl --deps=all | wright build --force
```

Example: Rebuild all reverse dependents (ABI-sensitive), then install the
resulting archives from stdin.

```bash
wright resolve zlib --rdeps=all --depth=0 | wright build --force --print-parts | wright install
```

**Install Origin Tracking**
Wright tracks why each part was installed using the `origin` field:

- `manual`: The user directly requested this part via `wright install` — never auto-removable.
- `dependency`: Automatically pulled in to satisfy another part's dependencies — eligible for orphan cleanup.

This distinction powers two features:

- `wright remove --cascade`: When removing a part, also remove its dependencies that have `dependency` origin and are no longer needed by any other part.
- `wright list --orphans`: Show `dependency`-origin parts that are no longer needed.

Origins follow a promotion hierarchy: `dependency → manual`. If you explicitly
install or apply a part that was previously pulled in as a dependency, Wright
promotes it to `manual`. Upgrading via `wright upgrade` preserves the existing
origin.

If you want a deeper view that maps these steps to code paths, see `docs/architecture.md`.
