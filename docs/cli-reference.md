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

Remove installed packages by name.

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
| `--stage <STAGE>` | Stop after a specific lifecycle stage |
| `--clean` | Clean build directory before building |
| `--lint` | Validate plan syntax only |
| `--rebuild` | Clean + force (full rebuild) |
| `--force` (`-f`) | Overwrite existing archives |
| `--update` | Download sources and update sha256 checksums |
| `-j`/`--jobs <N>` | Parallel builds (default: 1) |

**Examples:**

```
wright build nginx                     # by name
wright build /var/hold/extra/nginx     # by path
wright build @base-system              # assembly
wright build --update nginx            # update checksums
wright build --lint nginx              # validate only
wright build -j4 @desktop             # parallel
```

Archives are placed in the components directory. Build logs: `/tmp/wright-build/<name>-<version>/log/<stage>.log`.
