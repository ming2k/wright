# CLI Reference

Wright is split into three specialized tools:

| Tool | Role |
|------|------|
| **`wbuild`** | Part constructor — build and validate plans |
| **`wrepo`** | Repository manager — index, search, source configuration |
| **`wright`** | System administrator — install, remove, upgrade, query |

Running `cargo build` or `cargo build --release` also generates man pages for these tools in `target/man/`.
To install them for `man(1)`:

```sh
install -Dm644 target/man/wright.1 /usr/share/man/man1/wright.1
install -Dm644 target/man/wbuild.1 /usr/share/man/man1/wbuild.1
install -Dm644 target/man/wrepo.1 /usr/share/man/man1/wrepo.1
```

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

Install parts. Each argument can be a `.wright.tar.zst` file path, a part name (resolved from configured sources), or a `@kit` reference (expands to all parts in the named kit). Multiple kits can be combined freely — they are non-dependent, combinatory groupings and overlapping parts are deduplicated. Transactional — failures are rolled back. Handles `replaces` and `conflicts` automatically.

Parts explicitly listed by the user are marked as `explicit`; dependencies pulled in automatically are marked as `dependency`. If a part was previously installed as a dependency, explicitly installing it again promotes it to `explicit` so it won't be removed by cascade operations.

| Flag | Description |
|------|-------------|
| `--force` | Reinstall even if already installed; overwrite conflicting files |
| `--nodeps` | Skip dependency checks |

```bash
wright install zlib
wright install zlib openssl
wright install @base-devel
wright install ./zlib-1.3.1-1-x86_64.wright.tar.zst
```

#### `wright upgrade <PACKAGES...>`

Upgrade installed parts by name or from archive files. When given a part name, the resolver searches all configured sources for available versions and picks the latest. When given a file path, upgrades directly from that archive.

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

Upgrade all installed parts to the latest available versions found by the resolver.

| Flag | Description |
|------|-------------|
| `--dry-run` (`-n`) | Preview what would be upgraded without making changes |

```bash
wright sysupgrade
wright sysupgrade --dry-run
```

#### `wright remove <PACKAGES...>`

Remove installed parts by name. Refuses to remove a part if other installed parts depend on it. **Link dependencies** provide CRITICAL protection and block removal unless `--force` is used.

| Flag | Description |
|------|-------------|
| `--force` | Remove even if other installed parts depend on this one (bypasses safety) |
| `--recursive` (`-r`) | Also remove all installed parts that depend on the target (leaf-first order) |
| `--cascade` (`-c`) | Also remove orphan dependencies — auto-installed parts that are no longer needed by anything else |

```bash
wright remove zlib
wright remove zlib --recursive
wright remove zlib --cascade
```

#### `wright deps [PACKAGE]`

Analyze dependency relationships of **installed** parts.

| Flag | Description |
|------|-------------|
| `--reverse` (`-r`) | Show reverse dependencies (what depends on this part) |
| `--depth <N>` (`-d`) | Maximum tree depth (0 = unlimited, default: 0) |
| `--filter <PATTERN>` (`-f`) | Only show parts whose name contains the pattern |
| `--all` (`-a`) | Show dependency tree for all installed parts |
| `--prefix <MODE>` | Output prefix style: `indent`, `depth`, or `none` |
| `--prune <PACKAGE>` | Hide the subtree of the named part; may be repeated |

```bash
wright deps zlib
wright deps zlib --reverse
wright deps --all --depth 2
wright deps zlib --prefix depth
```

#### `wright doctor`

Perform a full system health check. Diagnoses:
- Database integrity
- Dependency satisfaction (broken or missing deps)
- Circular dependencies
- File ownership conflicts
- Recorded forced overwrites (shadows)

#### `wright list`

List installed parts.

| Flag | Description |
|------|-------------|
| `--roots` (`-r`) | Show only top-level parts with no installed dependents |
| `--assumed` (`-a`) | Show only assumed (externally provided) parts |
| `--orphans` (`-o`) | Show only orphan parts — auto-installed dependencies no longer needed by any part |

```bash
wright list
wright list --roots
wright list --orphans
wright list --assumed
```

#### `wright query <PACKAGE>`

Show detailed info for an installed part.

```bash
wright query zlib
```

#### `wright search <KEYWORD>`

Search installed parts by keyword (name and description). Use `wrepo search` to search available indexed parts.

```bash
wright search ssl
wright search python
```

#### `wright files <PACKAGE>`

List files owned by a part.

```bash
wright files zlib
```

#### `wright owner <FILE>`

Find which part owns a file.

```bash
wright owner /usr/bin/awk
wright owner /usr/lib/libz.so
```

#### `wright assume <NAME> <VERSION>`

Register an externally-provided part so that dependency checks treat it as satisfied. No files are tracked — wright will not manage, verify, or remove any files for an assumed part.

This is intended for **bootstrapping scenarios** where core system parts (glibc, gcc, binutils, etc.) already exist on the target but were not installed through wright. Without assuming them, installing any part that lists them as dependencies would fail with an unresolved dependency error.

Assuming a part is **idempotent** — running it again with a different version simply updates the recorded version.

Assumed parts are shown with an `[external]` tag in `wright list`.

When a real part is installed via `wright install`, any existing assumed record for that part is **automatically replaced** — no manual removal is needed.

```bash
wright assume glibc 2.41
wright assume gcc 15.1.0
```

#### `wright unassume <NAME>`

Remove an assumed part record. This only works on parts marked as assumed (i.e. registered via `wright assume`); it will not remove normally installed parts.

```sh
wright unassume glibc
```

#### `wright verify [PACKAGE]`

Verify installed file integrity (SHA-256 checksums). Omit the part name to verify all installed parts. For a full dependency and integrity health check, use `wright doctor`.

```bash
wright verify
wright verify zlib
```

#### `wright history [PACKAGE]`

Show part transaction history. Omit the part name to show all recorded transactions.

```bash
wright history
wright history zlib
```

---

## Wbuild (Part Constructor)

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

Build parts from `plan.toml` files. Targets can be plan names, paths, or `@assemblies`. Assemblies are non-dependent, combinatory groupings — multiple assemblies can be combined freely and overlapping plans are deduplicated. When used with `--install` (`-i`), parts already installed on the system are automatically skipped.

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Run only the specified lifecycle stage; may be repeated to run multiple stages in pipeline order (e.g. `--stage check --stage staging --stage fabricate`). Skips fetch/verify/extract — requires a previous full build. Omit entirely to run the full pipeline. |
| `--clean` | Clear the build cache entry, working directory, and source tree before starting. Without `--clean`, the source tree (`src/`) is preserved across builds when the build key is unchanged, enabling incremental compilation. `--clean` forces a full re-extraction and recompile. Composable with `--force`. |
| `--force` (`-f`) | Bypass the output archive skip check and always rebuild. Does not delete the build cache — use `--clean --force` to also clear the cache and fully start from scratch. |
| `-w` / `--dockyards <N>` | Max concurrent dockyard processes (0 = auto = available_cpus − 4, minimum 1). Only parts with no dependency relationship run simultaneously. Controls part-level concurrency — compiler-level parallelism inside each dockyard is set by CPU affinity (`nproc` returns the correct count automatically). See [Resource Allocation](resource-allocation.md) for details. |
| `--install` (`-i`) | Automatically install each built part after success. User-specified targets are marked `explicit`; auto-resolved dependencies are marked `dependency`. Upgrading an already-installed part preserves its existing install reason — use `wright install` to promote a dependency to explicit. |
| `--mvp` | Build using the `[mvp.dependencies]` dep set; sets `WRIGHT_BUILD_PHASE=mvp` without requiring a dependency cycle |

##### Expansion scope

These three flags control **which parts** are added to the build set. They are additive and composable. When none are given, the default applies.

| Flag | Description |
|------|-------------|
| `--self` (`-s`) | Include the listed targets themselves |
| `--deps` (`-d`) | Include missing upstream dependencies (build + link, not yet installed) |
| `--dependents` | Include parts that link against the target |

| Flags used | Listed targets | Missing upstream deps | Downstream link cascade |
|------------|-----------------|-----------------------|------------------------|
| (default) | ✓ | ✓ | ✗ |
| `--self` | ✓ | ✗ | ✗ |
| `--deps` | ✗ | ✓ | ✗ |
| `--dependents` | ✗ | ✗ | ✓ |
| `--self --deps` | ✓ | ✓ | ✗ |
| `--self --dependents` | ✓ | ✗ | ✓ |
| `--self --deps --dependents` | ✓ | ✓ | ✓ |

##### Force-rebuild modifiers

These two flags are **force-rebuild escalators** — they extend the scope of the corresponding expansion flags to include parts that would otherwise be skipped (already installed or non-link dependents).

| Flag | What it does | Compared to its scope counterpart |
|------|--------------|-----------------------------------|
| `-D` / `--rebuild-dependencies` | Force-rebuild ALL upstream dependencies, including already-installed ones | Like `--deps` but does not skip installed parts |
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

# gtk4 already updated — cascade rebuild to parts that link against it, skip gtk4 itself
wbuild run gtk4 --dependents

# Rebuild gtk4 AND cascade to its link-dependents (full ABI rebuild)
wbuild run gtk4 --self --dependents

# Everything: deps + self + cascade
wbuild run gtk4 --self --deps --dependents

# Force-rebuild gtk4 and ALL its deps, even installed ones (deep clean)
wbuild run gtk4 --deps -D

# gtk4 ABI changed, force-rebuild every part that depends on it (not just link deps)
wbuild run gtk4 --dependents -R

# Build freetype using its [mvp.dependencies] set (e.g. to test the MVP phase manually)
wbuild run freetype --mvp

# MVP build, run only up to the configure stage
wbuild run freetype --mvp --stage configure
```

##### Compile-stage serialization

When multiple dockyards run in parallel, non-compile stages (configure, staging, fabricate, etc.) execute concurrently with CPU cores partitioned across active builds. However, **compile stages are serialized** behind a semaphore — only one dockyard compiles at a time, and the active compile gets access to all available CPU cores.

This eliminates the "long-tail effect" where light parts finish quickly and leave their allocated cores idle while heavy compiles (python, perl, gcc) continue with only a fraction of available cores. The result is better CPU utilization and faster wall-clock times for multi-part builds.

The behavior is automatic and requires no configuration.

##### Output control

By default `wbuild run` is quiet about subprocess I/O — build tool output (make, cmake, etc.) is captured to per-stage `.log` files only. The **Construction Plan** and per-part `[done]` completion lines are written to stderr.

| Mode | Subprocess output | Construction Plan / done lines | Log level |
|------|:-----------------:|:-----------------------------:|-----------:|
| default, single dockyard | echoed to terminal (auto) | shown | info |
| default, multiple dockyards | captured only | shown | info |
| `--verbose` (`-v`), single dockyard | echoed to terminal | shown | debug |
| `--verbose` (`-v`), multiple dockyards | echoed to terminal (may interleave) | shown | debug |
| `--quiet` | captured only | hidden | warn |

Before building, `wbuild run` displays a **Construction Plan** listing all parts to be built and the reason:

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

Import a directory of `.wright.tar.zst` archives into the repository SQLite catalog. Defaults to `components_dir` (`/var/lib/wright/components`) if no directory is given.

```bash
wrepo sync                              # index the default components_dir
wrepo sync ./components                 # index a specific directory
```

#### `wrepo list [NAME]`

List all parts in the repository catalog. If a name is given, shows all available versions of that part. Installed versions are marked with `[installed]`.

```bash
wrepo list                   # list all indexed parts
wrepo list zlib              # show all available versions of zlib
```

#### `wrepo search <KEYWORD>`

Search available parts in the repository catalog by keyword (name and description). Installed parts are marked with `[installed]`.

```bash
wrepo search zlib
wrepo search ssl
```

#### `wrepo remove <NAME> <VERSION> [--purge]`

Remove a part entry from the repository catalog. The version can include a release number (e.g. `1.2.3-2`); without a release, all releases of that version are removed.

| Flag | Description |
|------|-------------|
| `--purge` | Also delete the `.wright.tar.zst` archive file from disk |

```bash
wrepo remove zlib 1.3.1               # remove from index only
wrepo remove zlib 1.3.1-2 --purge     # remove from index and delete archive
```

#### `wrepo source add <NAME> --path <PATH>`

Add a new local repository source to `/etc/wright/repos.toml`. Higher priority sources are preferred during resolution.

| Flag | Description |
|------|-------------|
| `--path <PATH>` | Local directory path (required) |
| `--priority <N>` | Priority — higher number is preferred (default: `100`) |

```bash
wrepo source add local --path /srv/wright/repo
wrepo source add cache --path ./repo --priority 200
```

#### `wrepo source remove <NAME>`

Remove a repository source from `/etc/wright/repos.toml`.

```bash
wrepo source remove local
```

#### `wrepo source list`

List all configured repository sources with their type, priority, and path.

```bash
wrepo source list
```
