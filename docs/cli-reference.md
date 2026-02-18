# CLI Reference

```
wright [OPTIONS] <COMMAND>
```

### Global Options

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Alternate root directory for file operations (default: `/`) |
| `--config <PATH>` | Path to config file |
| `--db <PATH>` | Path to database file |

## Package Management

#### `wright install <PACKAGES...>`

Install from local `.wright.tar.zst` files. Transactional — failures are rolled back.

| Flag | Description |
|------|-------------|
| `--force` | Reinstall even if already installed |
| `--nodeps` | Skip dependency checks |

#### `wright upgrade <PACKAGES...>`

Upgrade from local `.wright.tar.zst` files.

| Flag | Description |
|------|-------------|
| `--force` | Allow downgrades |

#### `wright remove <PACKAGES...>`

Remove installed packages by name. Refuses to remove a package if other installed packages depend on it. If a package is a **link dependency** of another, removal is blocked with a CRITICAL error. Use `--force` to override (dangerous!), or `--recursive` to remove the entire dependency chain.

| Flag | Description |
|------|-------------|
| `--force` | Remove even if other packages depend on this one (ignores link safety) |
| `--recursive` (`-r`) | Also remove all packages that depend on the target (leaf-first order) |

**Examples:**

```
wright remove nginx                  # fails if other packages depend on nginx
wright remove openssl                # CRITICAL failure if link-dependents exist
wright remove openssl --recursive    # remove openssl + everything that depends on it
```

#### `wright deps [PACKAGE]`

Analyze package dependency relationships. Displays a tree view. Link dependencies are marked with `[link]`.

| Flag | Description |
|------|-------------|
| `--reverse` (`-r`) | Show reverse dependencies (what depends on this package) |
| `--depth <N>` (`-d`) | Maximum tree depth (0 = unlimited, default: 0) |
| `--filter <PATTERN>` (`-f`) | Only show packages whose name contains the pattern |
| `--tree` (`-t`) | Show full system dependency tree (all packages) |

**Examples:**

```
wright deps openssl                  # what does openssl depend on?
wright deps openssl --reverse        # what depends on openssl?
wright deps --tree                   # show full system hierarchy
```

**Output:**

```
openssl
├── zlib [link]
└── perl (>= 5.10) [not installed]
```

```
openssl        (--reverse)
├── curl [link]
│   └── git
├── nginx [link]
└── python
```

Uninstalled dependencies are marked `[not installed]`. Version constraints are shown in parentheses.

#### `wright list`

List installed packages. Output: `name version-release (arch)`

| Flag | Description |
|------|-------------|
| `--roots` | Show only top-level packages (those not required by any other package) |

#### `wright query <PACKAGE>`

Show detailed info for an installed package.

#### `wright search <KEYWORD>`

Search installed packages by keyword.

#### `wright files <PACKAGE>`

List files owned by a package.

#### `wright owner <FILE>`

Find which package owns a file.

#### `wright verify [PACKAGE]`

Verify installed file integrity (SHA-256). Verifies all packages if none specified.

| Flag | Description |
|------|-------------|
| `--check-deps` | Check for broken dependencies across the whole system |

#### `wright doctor`

Perform a comprehensive system health check. This command diagnoses:
- **Database Integrity**: Physical check of the SQLite database file.
- **Dependency Sanity**: Missing dependencies, broken links, and version mismatches.
- **Circular Dependencies**: Detects logic errors in the dependency graph.
- **File Ownership**: Identifies file conflicts where multiple packages claim the same file.

Use this command if you suspect system corruption or after performing many `--force` operations.

---

## Build

#### `wright build [TARGETS]...`

Build packages from `plan.toml` files. Targets can be plan directories, plan names, or `@assembly` names.

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Stop after a specific lifecycle stage (preserves build dir for inspection) |
| `--only <STAGE>` | Run only a single stage, preserving `src/` from a previous build |
| `--clean` | Clean build directory before building |
| `--lint` | Validate plan syntax only |
| `--force` (`-f`) | Overwrite existing archives |
| `--update` | Download sources and update sha256 checksums |
| `-j`/`--jobs <N>` | Parallel builds (default: 1) |
| `--rebuild-dependents` (`-R`) | Also rebuild all packages that depend on the target (transitive) |
| `--install` (`-i`) | Automatically install each package after a successful build |

Before building, wright displays a **Construction Plan** showing all targets and their rebuild reasons:
- `[NEW]`: Explicitly requested targets or auto-resolved missing dependencies.
- `[LINK-REBUILD]`: Automatic rebuild due to a `link` dependency update.
- `[REV-REBUILD]`: Transitive rebuild requested via `--rebuild-dependents`.

Each build starts from a clean state (source re-extracted, all stages re-run) to ensure reproducibility. Downloaded sources are cached and reused across builds. Use `--stage` to stop early or `--only` to rerun a single stage — see [usage.md](usage.md#staged-builds) for the staged build workflow.

**Examples:**

```
wright build nginx                     # by name
wright build /var/hold/extra/nginx     # by path
wright build @base-system              # assembly
wright build --update nginx            # update checksums
wright build --lint nginx              # validate only
wright build --stage configure nginx   # stop after configure for debugging
wright build --only compile nginx      # rerun just the compile stage
wright build -j4 @desktop             # parallel
wright build openssl --rebuild-dependents    # rebuild openssl + all its dependents
wright build openssl -R -j4                  # same, with parallel rebuild
wright build curl --install            # build curl, auto-resolve missing deps, and install all
```

`--install` (or `-i`) is the most convenient way to build and install a package. Wright will recursively find all missing build/link dependencies in the hold tree, add them to the construction plan, and install them immediately after they are built so that the next packages in the queue can link against them.

`--rebuild-dependents` is designed for ABI breakage scenarios: when a library is updated and all packages linked against it need to be rebuilt. wright **automatically** includes `link` dependents in the build set even without this flag. The flag expands this behavior to all dependency types (runtime and build).

Archives are placed in the components directory. Build logs: `/tmp/wright-build/<name>-<version>/log/<stage>.log`.
