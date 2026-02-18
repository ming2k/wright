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

Remove installed packages by name. Refuses to remove a package if other installed packages depend on it (use `--force` to override, or `--recursive` to remove the entire dependency chain).

| Flag | Description |
|------|-------------|
| `--force` | Remove even if other packages depend on this one |
| `--recursive` (`-r`) | Also remove all packages that depend on the target (leaf-first order) |

**Examples:**

```
wright remove nginx                  # fails if other packages depend on nginx
wright remove nginx --force          # remove anyway, leaving broken deps
wright remove openssl --recursive    # remove openssl + everything that depends on it
```

#### `wright deps <PACKAGE>`

Analyze package dependency relationships. Displays a tree view.

| Flag | Description |
|------|-------------|
| `--reverse` (`-r`) | Show reverse dependencies (what depends on this package) |
| `--depth <N>` (`-d`) | Maximum tree depth (0 = unlimited, default: 0) |
| `--filter <PATTERN>` (`-f`) | Only show packages whose name contains the pattern |

**Examples:**

```
wright deps openssl                  # what does openssl depend on?
wright deps openssl --reverse        # what depends on openssl?
wright deps openssl -r -d 1          # direct reverse dependents only
wright deps openssl -r -f curl       # reverse deps, filter to "curl"
```

**Output:**

```
openssl
├── zlib
└── perl (>= 5.10) [not installed]
```

```
openssl        (--reverse)
├── curl
│   └── git
├── nginx
└── python
```

Uninstalled dependencies are marked `[not installed]`. Version constraints are shown in parentheses.

#### `wright list`

List installed packages. Output: `name version-release (arch)`

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
| `--rebuild-deps` | After building the target, also rebuild all packages that depend on it (transitive) |

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
wright build openssl --rebuild-deps    # rebuild openssl + all its dependents
wright build openssl --rebuild-deps -j4  # same, with parallel rebuild
```

`--rebuild-deps` is designed for ABI breakage scenarios: when a library is updated and all packages linked against it need to be rebuilt. The build system scans all known plans, transitively collects every package that depends (runtime or build) on the target, and includes them in the build set. The existing dependency-aware build orchestrator ensures correct build order.

Archives are placed in the components directory. Build logs: `/tmp/wright-build/<name>-<version>/log/<stage>.log`.
