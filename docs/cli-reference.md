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

### Commands

#### `wbuild run [TARGETS]...`

Build packages from `plan.toml` files. Targets can be plan names, paths, or `@assemblies`.

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Stop after a specific lifecycle stage |
| `--only <STAGE>` | Run only a single stage |
| `--clean` | Clean build directory before building |
| `--force` (`-f`) | Overwrite existing archives |
| `-j`/`--jobs <N>` | Parallel builds |
| `--rebuild-dependents` (`-R`) | Rebuild packages that depend on the target (downward) |
| `--rebuild-dependencies` (`-D`) | Rebuild packages that the target depends on (upward) |
| `--install` (`-i`) | Automatically install each package after a successful build |
| `--depth <N>` | Maximum recursion depth for `-D` and `-R` (default: 1) |

#### `wbuild check [TARGETS]...`

Validate `plan.toml` files for syntax and logic errors.

#### `wbuild fetch [TARGETS]...`

Download and cache sources for the specified plans without building.

#### `wbuild deps <TARGET>`

Analyze the **static** dependency tree of a plan in the hold tree. Shows what *would* be built.

#### `wbuild update [TARGETS]...`

Download sources and update SHA256 checksums in `plan.toml`.