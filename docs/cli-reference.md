# CLI Reference

Wright is split into three specialized tools:

| Tool | Role |
|------|------|
| **`wbuild`** | Package constructor — build and validate plans |
| **`wrepo`** | Repository manager — index, search, source configuration |
| **`wright`** | System administrator — install, remove, upgrade, query |

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

Install packages. Each argument can be a `.wright.tar.zst` file path, a package name (resolved from configured sources), or a `@kit` reference (expands to all packages in the named kit). Multiple kits can be combined freely — they are non-dependent, combinatory groupings and overlapping packages are deduplicated. Transactional — failures are rolled back. Handles `replaces` and `conflicts` automatically.

Packages explicitly listed by the user are marked as `explicit`; dependencies pulled in automatically are marked as `dependency`. If a package was previously installed as a dependency, explicitly installing it again promotes it to `explicit` so it won't be removed by cascade operations.

| Flag | Description |
|------|-------------|
| `--force` | Reinstall even if already installed; overwrite conflicting files |
| `--nodeps` | Skip dependency checks |

#### `wright upgrade <PACKAGES...>`

Upgrade installed packages by name or from archive files. When given a package name, the resolver searches all configured sources for available versions and picks the latest. When given a file path, upgrades directly from that archive.

| Flag | Description |
|------|-------------|
| `--force` | Allow downgrades or same-version reinstalls |
| `--version <VERSION>` | Target a specific version instead of the latest (implies `--force` for downgrades) |

```bash
wright upgrade gcc                          # upgrade to latest available version
wright upgrade gcc --version 14.2.0         # downgrade/switch to a specific version
wright upgrade gcc-15.1.0-1-x86_64.wright.tar.zst  # upgrade from a file (still works)
```

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
| `--cascade` (`-c`) | Also remove orphan dependencies — auto-installed packages that are no longer needed by anything else |

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
| `--orphans` (`-o`) | Show only orphan packages — auto-installed dependencies no longer needed by any package |

#### `wright query <PACKAGE>`

Show detailed info for an installed package.

#### `wright search <KEYWORD>`

Search installed packages by keyword (name and description). Use `wrepo search` to search available (indexed) packages.

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

Build packages from `plan.toml` files. Targets can be plan names, paths, or `@assemblies`. Assemblies are non-dependent, combinatory groupings — multiple assemblies can be combined freely and overlapping plans are deduplicated. When used with `--install` (`-i`), packages already installed on the system are automatically skipped.

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Run only the specified lifecycle stage; may be repeated to run multiple stages in pipeline order (e.g. `--stage check --stage staging --stage fabricate`). Skips fetch/verify/extract — requires a previous full build. Omit entirely to run the full pipeline. |
| `--clean` | Clear the build cache entry, working directory, and source tree before starting. Without `--clean`, the source tree (`src/`) is preserved across builds when the build key is unchanged, enabling incremental compilation. `--clean` forces a full re-extraction and recompile. Composable with `--force`. |
| `--force` (`-f`) | Bypass the output archive skip check and always rebuild. Does not delete the build cache — use `--clean --force` to also clear the cache and fully start from scratch. |
| `-w` / `--dockyards <N>` | Max concurrent dockyard processes (0 = auto = available_cpus − 4, minimum 1). Only packages with no dependency relationship run simultaneously. Controls package-level concurrency — compiler-level parallelism inside each dockyard is set by CPU affinity (`nproc` returns the correct count automatically). See [Resource Allocation](resource-allocation.md) for details. |
| `--install` (`-i`) | Automatically install each package after a successful build. User-specified targets are marked `explicit`; auto-resolved dependencies are marked `dependency`. Upgrading an already-installed package preserves its existing install reason — use `wright install` to promote a dependency to explicit. |
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

When multiple dockyards run in parallel, non-compile stages (configure, staging, fabricate, etc.) execute concurrently with CPU cores partitioned across active builds. However, **compile stages are serialized** behind a semaphore — only one dockyard compiles at a time, and the active compile gets access to all available CPU cores.

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

---

## Wrepo (Repository Manager)

```
wrepo [OPTIONS] <COMMAND>
```

### Global Options

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Path to config file |
| `-v` / `--verbose` | Increase log verbosity; use twice (`-vv`) for trace-level logs |
| `--quiet` | Reduce output to warnings and errors only |

### Commands

#### `wrepo sync [DIR]`

Scan a directory of `.wright.tar.zst` archives and generate or update `wright.index.toml`. Defaults to `components_dir` (`/var/lib/wright/components`) if no directory is given.

```bash
wrepo sync                              # index the default components_dir
wrepo sync /var/lib/wright/myrepo       # index a specific directory
```

#### `wrepo list [NAME]`

List all parts in the repository index. If a name is given, shows all available versions of that part. Installed versions are marked with `[installed]`.

```bash
wrepo list                   # list all indexed parts
wrepo list gcc               # show all available versions of gcc
```

#### `wrepo search <KEYWORD>`

Search available (indexed) packages by keyword (name and description). Installed packages are marked with `[installed]`.

```bash
wrepo search curl
```

#### `wrepo remove <NAME> <VERSION> [--purge]`

Remove a part entry from the repository index. The version can include a release number (e.g. `1.2.3-2`); without a release, all releases of that version are removed.

| Flag | Description |
|------|-------------|
| `--purge` | Also delete the `.wright.tar.zst` archive file from disk |

```bash
wrepo remove gcc 14.2.0-2             # remove from index only
wrepo remove gcc 14.2.0-2 --purge     # remove from index and delete archive
```

#### `wrepo source add <NAME> --path <PATH>`

Add a new local repository source to `/etc/wright/repos.toml`.

| Flag | Description |
|------|-------------|
| `--type <TYPE>` | Source type: `local` or `hold` (default: `local`) |
| `--path <PATH>` | Local directory path (required) |
| `--priority <N>` | Priority — higher number is preferred (default: `100`) |

```bash
wrepo source add myrepo --path /var/lib/wright/myrepo
wrepo source add myrepo --path /var/lib/wright/myrepo --priority 300
```

#### `wrepo source remove <NAME>`

Remove a repository source from `/etc/wright/repos.toml`.

#### `wrepo source list`

List all configured repository sources with their type, priority, and path.
