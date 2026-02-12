# CLI Reference

wright provides three binaries: `wright` (package manager), `wright-build` (build tool), and `wright-repo` (repository tool).

## wright

Package manager for installing, removing, and querying packages.

```
wright [OPTIONS] <COMMAND>
```

### Global Options

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Alternate root directory for file operations (default: `/`) |
| `--config <PATH>` | Path to config file (default: `/etc/wright/wright.toml`) |
| `--db <PATH>` | Path to database file (default: from config) |

### Commands

#### `wright install <PACKAGES...>`

Install packages from local `.wright.tar.zst` archive files.

```
wright install hello-1.0.0-1-x86_64.wright.tar.zst
wright install pkg1.wright.tar.zst pkg2.wright.tar.zst
```

The install operation is transactional — if any package fails to install, all changes from that package are rolled back.

#### `wright remove <PACKAGES...>`

Remove installed packages by name.

```
wright remove hello
wright remove nginx openssl
```

Removes all files owned by the package and deletes the database record. The removal is transactional.

#### `wright list [--installed]`

List all installed packages. The `--installed` flag is accepted but currently the default (and only) behavior.

```
wright list
```

Output format: `name version-release (arch)`

#### `wright query <PACKAGE>`

Show detailed information about an installed package.

```
wright query hello
```

Displays: name, version, release, description, architecture, license, URL, install size, install date, and package hash.

#### `wright search <KEYWORD>`

Search installed packages by keyword (matches against name and description).

```
wright search http
```

#### `wright files <PACKAGE>`

List all files owned by an installed package.

```
wright files hello
```

#### `wright owner <FILE>`

Find which installed package owns a given file path.

```
wright owner /usr/bin/hello
```

#### `wright verify [PACKAGE]`

Verify the integrity of installed package files by checking SHA-256 hashes. If no package name is given, verifies all installed packages.

```
wright verify hello
wright verify
```

Reports missing files, hash mismatches, and other integrity issues.

---

## wright-build

Build tool that parses `package.toml` files, executes the lifecycle pipeline, and produces binary package archives.

```
wright-build [OPTIONS] <HOLD_PATH>
```

### Arguments

| Argument | Description |
|----------|-------------|
| `<HOLD_PATH>` | Path to hold directory (containing `package.toml`) or directly to a `package.toml` file |

### Options

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Stop after a specific lifecycle stage |
| `--clean` | Clean build directory before building |
| `--lint` | Validate `package.toml` syntax only (no build) |
| `--rebuild` | Force rebuild (cleans first, then builds) |
| `--update` | Update `sha256` checksums in `package.toml` |
| `--config <PATH>` | Path to config file |

### Examples

Build a package:

```
wright-build /var/hold/extra/nginx
```

Update checksums for a package:

```
wright-build --update /var/hold/extra/nginx
```

Validate a package description without building:

```
wright-build --lint /var/hold/extra/nginx
```

Build only up to the configure stage:

```
wright-build --stage configure /var/hold/extra/nginx
```

Clean and rebuild:

```
wright-build --rebuild /var/hold/extra/nginx
```

### Build Output

On success, prints the path to the created `.wright.tar.zst` archive. The archive is placed in the current working directory.

Build logs are stored in the build directory (default: `/tmp/wright-build/<name>-<version>/log/`).

---

## wright-repo

Repository management tool for generating package indexes from built packages.

```
wright-repo
```

> **Note:** `wright-repo` is a placeholder in the current implementation. Repository index generation is planned for Phase 4 of development. See [design-spec.md](design-spec.md) for the full design.

### Planned Commands

```
wright-repo generate <PACKAGES_DIR> --output <REPO_DIR>
```

Will scan a directory of `.wright.tar.zst` files, extract metadata, compute checksums, and generate an `index.toml` repository index.

---

## Environment Variables

wright respects the `RUST_LOG` environment variable for controlling log output via the `tracing` framework:

```
RUST_LOG=info wright-build /var/hold/extra/hello
RUST_LOG=debug wright install hello-1.0.0-1-x86_64.wright.tar.zst
```
