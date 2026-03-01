# CLI Reference

Wright is split into two specialized tools:
- **`wright`**: System administrator for managing installed packages and system health.
- **`wbuild`**: Package constructor for building and validating plans.

---

## Wright (System Administrator)

```
wright [OPTIONS] <COMMAND>
```

### Global Options

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Alternate root directory for file operations (default: `/`) |
| `--config <PATH>` | Path to config file |
| `--db <PATH>` | Path to database file |
| `-v` / `--verbose` | Increase log verbosity; use twice (`-vv`) for trace-level logs |
| `--quiet` | Reduce output to warnings and errors only |

### Commands

#### `wright install <PACKAGES...>`

Install from local `.wright.tar.zst` files. Transactional — failures are rolled back. Handles `replaces` and `conflicts` automatically.

| Flag | Description |
|------|-------------|
| `--force` | Reinstall even if already installed; overwrite conflicting files |
| `--nodeps` | Skip dependency checks |

#### `wright upgrade <PACKAGES...>`

Upgrade from local `.wright.tar.zst` files.

| Flag | Description |
|------|-------------|
| `--force` | Allow downgrades |

#### `wright sysupgrade`

Upgrade all installed packages to the latest available versions found by the resolver.

| Flag | Description |
|------|-------------|
| `--dry-run` (`-n`) | Preview what would be upgraded without making changes |

#### `wright remove <PACKAGES...>`

Remove installed packages by name. Refuses to remove a package if other installed packages depend on it. **Link dependencies** provide CRITICAL protection and block removal unless `--force` is used.

| Flag | Description |
|------|-------------|
| `--force` | Remove even if other packages depend on this one (bypasses safety) |
| `--recursive` (`-r`) | Also remove all packages that depend on the target (leaf-first order) |

#### `wright deps [PACKAGE]`

Analyze dependency relationships of **installed** packages.

| Flag | Description |
|------|-------------|
| `--reverse` (`-r`) | Show reverse dependencies (what depends on this package) |
| `--depth <N>` (`-d`) | Maximum tree depth (0 = unlimited, default: 0) |
| `--filter <PATTERN>` (`-f`) | Only show packages whose name contains the pattern |
| `--all` (`-a`) | Show dependency tree for all installed packages |

#### `wright doctor`

Perform a full system health check. Diagnoses:
- Database integrity
- Dependency satisfaction (broken or missing deps)
- Circular dependencies
- File ownership conflicts
- Recorded forced overwrites (shadows)

#### `wright list`

List installed packages.

| Flag | Description |
|------|-------------|
| `--roots` (`-r`) | Show only top-level packages with no installed dependents |
| `--assumed` (`-a`) | Show only assumed (externally provided) packages |

#### `wright query <PACKAGE>`

Show detailed info for an installed package.

#### `wright search <KEYWORD>`

Search installed packages by keyword (name and description).

#### `wright files <PACKAGE>`

List files owned by a package.

#### `wright owner <FILE>`

Find which package owns a file.

#### `wright assume <NAME> <VERSION>`

Register an externally-provided package so that dependency checks treat it as satisfied. No files are tracked — wright will not manage, verify, or remove any files for an assumed package.

This is intended for **bootstrapping scenarios** where core system packages (glibc, gcc, binutils, etc.) already exist on the target but were not installed through wright. Without assuming them, installing any package that lists them as dependencies would fail with an unresolved dependency error.

Assuming a package is **idempotent** — running it again with a different version simply updates the recorded version.

Assumed packages are shown with an `[external]` tag in `wright list`.

When a real package is installed via `wright install`, any existing assumed record for that package is **automatically replaced** — no manual removal is needed.

#### `wright unassume <NAME>`

Remove an assumed package record. This only works on packages marked as assumed (i.e. registered via `wright assume`); it will not remove normally installed packages.

```sh
wright unassume glibc
```

#### `wright verify [PACKAGE]`

Verify installed file integrity (SHA-256 checksums). Omit the package name to verify all installed packages. For a full dependency and integrity health check, use `wright doctor`.

---

## Wbuild (Package Constructor)

```
wbuild [OPTIONS] <COMMAND> [TARGETS]...
```

### Global Options

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Alternate root directory for file operations |
| `--config <PATH>` | Path to config file |
| `--db <PATH>` | Path to database file |
| `--verbose` (`-v`) | Increase log verbosity; use twice (`-vv`) for trace-level logs |
| `--quiet` | Reduce output to warnings and errors only; suppresses Construction Plan and `[done]` messages |

### Commands

#### `wbuild run [TARGETS]...`

Build packages from `plan.toml` files. Targets can be plan names, paths, or `@assemblies`.

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Run only the specified lifecycle stage; may be repeated to run multiple stages in pipeline order (e.g. `--stage check --stage package`). Skips fetch/verify/extract — requires a previous full build. Omit entirely to run the full pipeline. |
| `--clean` | Clear the build cache entry and working directory before starting. The working directory is recreated at the start of every build anyway; the primary effect of `--clean` is invalidating the build cache so the next build must compile fully. Composable with `--force`. |
| `--force` (`-f`) | Bypass the output archive skip check and always rebuild. Does not delete the build cache — use `--clean --force` to also clear the cache and fully start from scratch. |
| `-w` / `--dockyards <N>` | Max concurrent dockyard processes (0 = auto = available_cpus − 4, minimum 1). Only packages with no dependency relationship run simultaneously. Controls package-level concurrency — compiler-level parallelism inside each dockyard is set by CPU affinity (`nproc` returns the correct count automatically). See [Resource Allocation](resource-allocation.md) for details. |
| `--install` (`-i`) | Automatically install each package after a successful build |
| `--mvp` | Build using the `[mvp.dependencies]` dep set; sets `WRIGHT_BUILD_PHASE=mvp` without requiring a dependency cycle |

##### Expansion scope

These three flags control **which packages** are added to the build set. They are additive and composable. When none are given, the default applies.

| Flag | Description |
|------|-------------|
| `--self` (`-s`) | Include the listed packages themselves |
| `--deps` (`-d`) | Include missing upstream dependencies (build + link, not yet installed) |
| `--dependents` | Include packages that link against the target |

| Flags used | Listed packages | Missing upstream deps | Downstream link cascade |
|------------|-----------------|-----------------------|------------------------|
| (default) | ✓ | ✓ | ✗ |
| `--self` | ✓ | ✗ | ✗ |
| `--deps` | ✗ | ✓ | ✗ |
| `--dependents` | ✗ | ✗ | ✓ |
| `--self --deps` | ✓ | ✓ | ✗ |
| `--self --dependents` | ✓ | ✗ | ✓ |
| `--self --deps --dependents` | ✓ | ✓ | ✓ |

##### Force-rebuild modifiers

These two flags are **force-rebuild escalators** — they extend the scope of the corresponding expansion flags to include packages that would otherwise be skipped (already installed or non-link dependents).

| Flag | What it does | Compared to its scope counterpart |
|------|--------------|-----------------------------------|
| `-D` / `--rebuild-dependencies` | Force-rebuild ALL upstream dependencies, including already-installed ones | Like `--deps` but does not skip installed packages |
| `-R` / `--rebuild-dependents` | Force-rebuild ALL downstream dependents, not just link dependents | Like `--dependents` but reaches runtime and build dependents too |

`-D` and `-R` can be combined with scope flags:
- `--deps -D`: add missing deps to build set AND force-rebuild installed deps
- `--dependents -R`: add link dependents AND force-rebuild non-link dependents too

| Flag | `--depth <N>` | Maximum expansion depth for all cascade operations (0 = unlimited) |
|------|---------------|----------------------------------------------------------------------|

**Examples:**

```bash
# Default: rebuild gtk4 + auto-resolve its missing deps (no downstream cascade)
wbuild run gtk4

# Only rebuild gtk4 itself — all deps assumed installed
wbuild run gtk4 --self

# Only build gtk4's missing deps — don't rebuild gtk4 itself (pre-stage deps)
wbuild run gtk4 --deps

# gtk4 already updated — cascade rebuild to packages that link against it, skip gtk4 itself
wbuild run gtk4 --dependents

# Rebuild gtk4 AND cascade to its link-dependents (full ABI rebuild)
wbuild run gtk4 --self --dependents

# Everything: deps + self + cascade
wbuild run gtk4 --self --deps --dependents

# Force-rebuild gtk4 and ALL its deps, even installed ones (deep clean)
wbuild run gtk4 --deps -D

# gtk4 ABI changed, force-rebuild every package that depends on it (not just link deps)
wbuild run gtk4 --dependents -R

# Build freetype using its [mvp.dependencies] set (e.g. to test the MVP phase manually)
wbuild run freetype --mvp

# MVP build, run only up to the configure stage
wbuild run freetype --mvp --stage configure
```

##### Compile-stage serialization

When multiple dockyards run in parallel, non-compile stages (configure, package, etc.) execute concurrently with CPU cores partitioned across active builds. However, **compile stages are serialized** behind a semaphore — only one dockyard compiles at a time, and the active compile gets access to all available CPU cores.

This eliminates the "long-tail effect" where light packages finish quickly and leave their allocated cores idle while heavy compiles (python, perl, gcc) continue with only a fraction of available cores. The result is better CPU utilization and faster wall-clock times for multi-package builds.

The behavior is automatic and requires no configuration.

##### Output control

By default `wbuild run` is quiet about subprocess I/O — build tool output (make, cmake, etc.) is captured to per-stage `.log` files only. The **Construction Plan** and per-package `[done]` completion lines are written to stderr.

| Mode | Subprocess output | Construction Plan / done lines | Log level |
|------|:-----------------:|:-----------------------------:|-----------:|
| default, single dockyard | echoed to terminal (auto) | shown | info |
| default, multiple dockyards | captured only | shown | info |
| `--verbose` (`-v`), single dockyard | echoed to terminal | shown | debug |
| `--verbose` (`-v`), multiple dockyards | echoed to terminal (may interleave) | shown | debug |
| `--quiet` | captured only | hidden | warn |

Before building, `wbuild run` displays a **Construction Plan** listing all packages to be built and the reason:

| Label | Meaning |
|-------|---------:|
| `[NEW]` | Explicitly requested target |
| `[LINK-REBUILD]` | Triggered because a link dependency was updated |
| `[REV-REBUILD]` | Triggered transitively via `-R` |
| `[MVP]` | MVP build: either a cycle-breaking first pass, or an explicit `--mvp` build |
| `[FULL]` | Second pass of a cycle build (complete rebuild after cycle is resolved) |

See [Phase-Based Cycles](writing-plans.md#phase-based-cycles-mvp--full) for details on the two-pass mechanism.

#### `wbuild check [TARGETS]...`

Validate `plan.toml` files for syntax and logic errors. Also prints a dependency graph analysis: whether the graph is acyclic, any detected cycles, and which MVP candidates would break each cycle.

#### `wbuild fetch [TARGETS]...`

Download and cache sources for the specified plans without building.

#### `wbuild deps <TARGET>`

Analyze the **static** dependency tree of a plan in the hold tree. Shows what *would* be built.

| Flag | Description |
|------|-------------|
| `--depth <N>` (`-d`) | Maximum tree depth (0 = unlimited, default: 0) |

#### `wbuild checksum [TARGETS]...`

Download sources and update SHA256 checksums in `plan.toml`. Only updates the specified plans — no dependency cascade is applied (unlike `wbuild run`, checksum is a per-plan metadata operation).
