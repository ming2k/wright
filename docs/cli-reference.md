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

Perform a full system health check. Diagnoses database integrity, dependency satisfaction, circular dependencies, file ownership conflicts, and records of forced overwrites (shadows).

#### `wright list`

List installed packages. Use `--roots` to show only top-level packages.

#### `wright query <PACKAGE>`

Show detailed info for an installed package.

#### `wright search <KEYWORD>`

Search installed packages by keyword.

#### `wright files <PACKAGE>`

List files owned by a package.

#### `wright owner <FILE>`

Find which package owns a file.

#### `wright verify [PACKAGE]`

Verify installed file integrity (SHA-256). Use `--check-deps` for a system-wide dependency check.

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
| `--stage <STAGE>` | Stop after a specific lifecycle stage |
| `--only <STAGE>` | Run only a single stage |
| `--clean` | Clean build directory before building |
| `--force` (`-f`) | Overwrite existing archives |
| `-j`/`--jobs <N>` | Parallel builds (0 = auto-detect) |
| `--rebuild-dependents` (`-R`) | Also rebuild packages that depend on the target (downward) |
| `--rebuild-dependencies` (`-D`) | Also rebuild packages that the target depends on (upward) |
| `--install` (`-i`) | Automatically install each package after a successful build |
| `--depth <N>` | Maximum recursion depth for `-D` and `-R` (default: 1) |
| `--self` (`-s`) | Include the listed packages themselves in the build |
| `--deps` (`-d`) | Include missing upstream dependencies (not the listed packages themselves) |
| `--dependents` | Include downstream link-rebuild dependents (not the listed packages themselves) |

##### Expansion scope

These three flags are **additive and composable**. When none are given, the default applies. When any are given, only the specified scopes are built.

| Flags used | Listed packages | Missing upstream deps | Downstream link cascade |
|------------|-----------------|-----------------------|------------------------|
| (default) | ✓ | ✓ | ✗ |
| `--self` | ✓ | ✗ | ✗ |
| `--deps` | ✗ | ✓ | ✗ |
| `--dependents` | ✗ | ✗ | ✓ |
| `--self --deps` | ✓ | ✓ | ✗ |
| `--self --dependents` | ✓ | ✗ | ✓ |
| `--self --deps --dependents` | ✓ | ✓ | ✓ |

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
```

`-D` and `-R` layer on top as force-rebuild modifiers: `-D` force-rebuilds all deps (even installed ones), `-R` force-rebuilds all dependents (not just link deps).

##### Output control

By default `wbuild run` is quiet about subprocess I/O — build tool output (make, cmake, etc.) is captured to per-stage `.log` files only. The **Construction Plan** and per-package `[done]` completion lines are written to stderr.

| Mode | Subprocess output | Construction Plan / done lines | Log level |
|------|:-----------------:|:-----------------------------:|-----------|
| default | captured only | shown | info |
| `--verbose` (`-v`) | echoed to terminal in real time | shown | debug |
| `--quiet` | captured only | hidden | warn |
| `-j >1` with `-v` | captured only (parallel builds suppress echo to avoid interleaving) | shown | debug |

Before building, `wbuild run` displays a **Construction Plan** listing all packages to be built and the reason:

| Label | Meaning |
|-------|---------|
| `[NEW]` | Explicitly requested target |
| `[LINK-REBUILD]` | Triggered because a link dependency was updated |
| `[REV-REBUILD]` | Triggered transitively via `-R` |
| `[MVP]` | First pass of a two-pass cycle build (built without cyclic dep) |
| `[FULL]` | Second pass of a cycle build (complete rebuild after cycle is resolved) |

`--exact` suppresses all automatic expansion; every package in the plan will be labelled `[NEW]`.

See [Phase-Based Cycles](writing-plans.md#phase-based-cycles-mvp--full) for details on the two-pass mechanism.

#### `wbuild check [TARGETS]...`

Validate `plan.toml` files for syntax and logic errors.

#### `wbuild fetch [TARGETS]...`

Download and cache sources for the specified plans without building.

#### `wbuild deps <TARGET>`

Analyze the **static** dependency tree of a plan in the hold tree. Shows what *would* be built.

#### `wbuild checksum [TARGETS]...`

Download sources and update SHA256 checksums in `plan.toml`. Only updates the specified plans — no dependency cascade is applied (unlike `wbuild run`, checksum is a per-plan metadata operation).
