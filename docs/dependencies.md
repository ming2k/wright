# Dependency Resolution (User View)

This document explains how Wright resolves and acts on dependencies from a user perspective. It focuses on what happens when you build and install parts, and how to interpret the output.

**Dependency Types**
Wright uses three dependency types, each with a different purpose.

- `build`: Required to compile a part but not necessarily needed at runtime.
- `link`: ABI-sensitive dependencies used by `wbuild` to trigger reverse rebuilds when a linked dependency changes.
- `runtime`: Required for the part to run after installation.

`link` and `runtime` are allowed to overlap. If something is needed after installation, it must be listed in `runtime` even if it also appears in `link`.

**Where Dependencies Come From**
Dependencies are declared in `plan.toml`:

- `dependencies.build`
- `dependencies.link`
- `dependencies.runtime`

Only `runtime` and part relations are serialized into binary part metadata used by `wright install`. `link` remains a build-graph concept used by `wbuild`.

You do not need to declare transitive dependencies. Wright expands them when you run builds that require it.

**What `wbuild run` Does With Dependencies**
`wbuild run` is the only command that performs dependency-driven expansion.

- It resolves your targets and prints a Construction Plan.
- It expands missing dependencies upward.
- It can expand reverse rebuilds downward.

**Upward Expansion: Missing Dependencies**
By default, `wbuild run` builds only the listed targets. Add `--deps` when you
want Wright to expand upstream dependencies from the hold tree.

- With `--deps` (or `--deps=missing`), missing `build` and `link` dependencies are added to the Construction Plan.
- With `--deps=sync`, missing dependencies and installed dependencies whose epoch/version/release differs from `plan.toml` are added.
- With `--install`, runtime dependencies are also considered while expanding upstream dependencies.
- If the dependency is missing and no plan exists, the build fails with a clear error.

With `-D` or `--rebuild-dependencies`, Wright expands more aggressively:

- `build`, `link`, and `runtime` dependencies are added to the plan.
- This is useful for deep rebuilds when you want a clean, consistent dependency chain.

**Downward Expansion: Reverse Rebuilds**
When a dependency changes, other parts may need to be rebuilt.

- `link` dependencies always trigger reverse rebuilds in `wbuild run`.
- `build` and `runtime` dependencies only trigger reverse rebuilds with `-R` or `--rebuild-dependents`.

This behavior keeps ABI-sensitive chains correct without forcing expensive rebuilds by default.

This rebuild behavior does not make `link` an implicit runtime dependency. Runtime requirements must still be declared in `runtime`.

**Construction Plan Labels**
`wbuild run` prints a Construction Plan. Each entry is labeled by why it is included.

- `[NEW]`: Explicitly requested, or missing dependency that had to be added.
- `[LINK-REBUILD]`: Rebuilt because a link dependency changed.
- `[REV-REBUILD]`: Rebuilt because of `-R` transitive expansion.
- `[MVP]`: Bootstrap build used to break a dependency cycle.
- `[FULL]`: Full build after an MVP bootstrap.

**Dependency Cycles and MVP Builds**
If Wright detects a dependency cycle, it tries to resolve it in a user-friendly way.

- If the part declares `mvp.dependencies` in `plan.toml`, Wright performs a two-pass build.
- The first pass is an **MVP build** (tagged `[MVP]` in the Construction Plan).
  It excludes the dependencies listed in `mvp.dependencies`.
- The second pass is a full build, forced to rebuild even if a partial archive exists.

This results in two builds for that part:

- `pkg` tagged `[MVP]`
- `pkg` tagged `[FULL]`

If no MVP definition exists, Wright stops and reports the cycle.

**Automatic Install With `wbuild run -i`**
If you pass `-i` or `--install`, Wright installs each part as soon as it finishes building.

- Installation is serialized to avoid parallel installs.
- The main part is installed first.
- Split parts are installed afterward, if they exist.

**Common Examples**
Example: Build and install with automatic dependency expansion.

```bash
wbuild run -i curl
```

Example: Force a deep rebuild of dependencies.

```bash
wbuild run -D openssl
```

Example: Rebuild all reverse dependents (ABI-sensitive).

```bash
wbuild run -R zlib
```

**Install Reason Tracking**
Wright tracks why each part was installed:

- `explicit`: The user directly requested this part via `wright install`.
- `dependency`: Automatically pulled in to satisfy another part's dependencies.

This distinction powers two features:

- `wright remove --cascade`: When removing a part, also remove its dependencies that were auto-installed and are no longer needed by any other part.
- `wright list --orphans`: Show auto-installed dependencies that are no longer needed.

If you explicitly install a part that was previously pulled in as a dependency, wright promotes it to `explicit` so it won't be affected by cascade removal. Existing parts (installed before this feature) default to `explicit`.

If you want a deeper view that maps these steps to code paths, see `docs/architecture.md`.
