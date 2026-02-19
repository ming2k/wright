# Dependency Resolution (User View)

This document explains how Wright resolves and acts on dependencies from a user perspective. It focuses on what happens when you build and install packages, and how to interpret the output.

**Dependency Types**
Wright uses three dependency types, each with a different purpose.

- `build`: Required to compile a package but not necessarily needed at runtime.
- `link`: ABI-sensitive dependencies. If a link dependency changes, reverse rebuilds may be triggered.
- `runtime`: Required for the package to run after installation.

**Where Dependencies Come From**
Dependencies are declared in `plan.toml`:

- `dependencies.build`
- `dependencies.link`
- `dependencies.runtime`

You do not need to declare transitive dependencies. Wright expands them when you run builds that require it.

**What `wbuild run` Does With Dependencies**
`wbuild run` is the only command that performs dependency-driven expansion.

- It resolves your targets and prints a Construction Plan.
- It expands missing dependencies upward.
- It can expand reverse rebuilds downward.

**Upward Expansion: Missing Dependencies**
When you run `wbuild run`, Wright checks your target’s `build` and `link` dependencies.

- If a dependency is already installed, it is not rebuilt.
- If it is missing but a plan exists in the hold tree, Wright adds it to the Construction Plan.
- If the dependency is missing and no plan exists, the build fails with a clear error.

With `-D` or `--rebuild-dependencies`, Wright expands more aggressively:

- `build`, `link`, and `runtime` dependencies are added to the plan.
- This is useful for deep rebuilds when you want a clean, consistent dependency chain.

**Downward Expansion: Reverse Rebuilds**
When a dependency changes, other packages may need to be rebuilt.

- `link` dependencies always trigger reverse rebuilds in `wbuild run`.
- `build` and `runtime` dependencies only trigger reverse rebuilds with `-R` or `--rebuild-dependents`.

This behavior keeps ABI-sensitive chains correct without forcing expensive rebuilds by default.

**Construction Plan Labels**
`wbuild run` prints a Construction Plan. Each entry is labeled by why it is included.

- `[NEW]`: Explicitly requested, or missing dependency that had to be added.
- `[LINK-REBUILD]`: Rebuilt because a link dependency changed.
- `[REV-REBUILD]`: Rebuilt because of `-R` transitive expansion.
- `[MVP]`: Bootstrap build used to break a dependency cycle.
- `[FULL]`: Full build after an MVP bootstrap.

**Dependency Cycles and MVP Builds**
If Wright detects a dependency cycle, it tries to resolve it in a user-friendly way.

- If the package declares `mvp.dependencies` in `plan.toml`, Wright performs a two-pass build.
- The first pass is `:bootstrap` (MVP). It excludes the dependencies listed in `mvp.dependencies`.
- The second pass is a full build, forced to rebuild even if a partial archive exists.

This results in two builds for that package:

- `pkg:bootstrap` (MVP)
- `pkg` (FULL)

If no MVP definition exists, Wright stops and reports the cycle.

**Automatic Install With `wbuild run -i`**
If you pass `-i` or `--install`, Wright installs each package as soon as it finishes building.

- Installation is serialized to avoid parallel installs.
- The main package is installed first.
- Split packages are installed afterward, if they exist.

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

If you want a deeper view that maps these steps to code paths, see `docs/architecture.md`.
