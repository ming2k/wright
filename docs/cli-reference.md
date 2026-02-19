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
| `--tree` (`-t`) | Show full system dependency tree (all packages) |

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

#### `wright query <PACKAGE>`

Show detailed info for an installed package.

#### `wright search <KEYWORD>`

Search installed packages by keyword (name and description).

#### `wright files <PACKAGE>`

List files owned by a package.

#### `wright owner <FILE>`

Find which package owns a file.

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
| `--until <STAGE>` | Run all stages up to and including this one, then stop (e.g. `configure`, `compile`) |
| `--only <STAGE>` | Run exactly one stage; all others are skipped (requires a previous full build) |
| `--clean` | Remove the build directory before starting |
| `--force` (`-f`) | Force rebuild: overwrite existing archive and bypass the build cache |
| `-j` / `--jobs <N>` | Parallel builds (0 = auto-detect CPU count) |
| `--install` (`-i`) | Automatically install each package after a successful build |

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
```

##### Output control

By default `wbuild run` is quiet about subprocess I/O — build tool output (make, cmake, etc.) is captured to per-stage `.log` files only. The **Construction Plan** and per-package `[done]` completion lines are written to stderr.

| Mode | Subprocess output | Construction Plan / done lines | Log level |
|------|:-----------------:|:-----------------------------:|-----------:|
| default | captured only | shown | info |
| `--verbose` (`-v`) | echoed to terminal | shown | debug |
| `--quiet` | captured only | hidden | warn |
| `-j >1` with `-v` | captured only (parallel — no interleaving) | shown | debug |

Before building, `wbuild run` displays a **Construction Plan** listing all packages to be built and the reason:

| Label | Meaning |
|-------|---------:|
| `[NEW]` | Explicitly requested target |
| `[LINK-REBUILD]` | Triggered because a link dependency was updated |
| `[REV-REBUILD]` | Triggered transitively via `-R` |
| `[MVP]` | First pass of a two-pass cycle build (built without cyclic dep) |
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
