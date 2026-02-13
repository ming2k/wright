# CLI Reference

wright provides three binaries: `wright` (package manager), `wright-build` (build tool), and `wright-repo` (repository tool).

## wright

Package manager for installing, removing, and querying packages.

```
wright [OPTIONS] <COMMAND>
```

### Global Options

| Flag              | Description                                                 |
|-------------------|-------------------------------------------------------------|
| `--root <PATH>`   | Alternate root directory for file operations (default: `/`) |
| `--config <PATH>` | Path to config file (default: `/etc/wright/wright.toml`)    |
| `--db <PATH>`     | Path to database file (default: from config)                |

### Commands

#### `wright install <PACKAGES...>`

Install packages from local `.wright.tar.zst` archive files.

```
wright install hello-1.0.0-1-x86_64.wright.tar.zst --force
wright install pkg1.wright.tar.zst pkg2.wright.tar.zst --nodeps
```

**Options:**

| Flag       | Description                                               |
|------------|-----------------------------------------------------------|
| `--force`  | Force reinstall even if the package is already installed. |
| `--nodeps` | Skip dependency resolution checks.                        |

The install operation is transactional — if any package fails to install, all changes from that package are rolled back.

#### `wright upgrade <PACKAGES...>`

Upgrade installed packages from local `.wright.tar.zst` archive files.

```
wright upgrade hello-1.0.1-1-x86_64.wright.tar.zst
```

**Options:**

| Flag      | Description                                     |
|-----------|-------------------------------------------------|
| `--force` | Force upgrade even if the version is not newer. |

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

Build tool that parses `package.toml` files, executes the lifecycle pipeline, and produces binary package archives. It supports building individual plans, directories, or entire assemblies.

```
wright-build [OPTIONS] [TARGETS]...
```

### Arguments

| Argument | Description |
|----------|-------------|
| `[TARGETS]...` | Paths to plan directories, part names (looked up in configured plan directories), or `@assembly` names. |

### Options

| Flag | Description |
|------|-------------|
| `--stage <STAGE>` | Stop after a specific lifecycle stage |
| `--clean` | Clean build directory before building |
| `--lint` | Validate `package.toml` syntax only (no build) |
| `--rebuild` | Force rebuild (cleans first, then builds) |
| `--force` (`-f`) | Force overwrite existing archive (skip check disabled) |
| `--update` | Update `sha256` checksums in `package.toml` |
| `--config <PATH>` | Path to config file |
| `--jobs <N>` (`-j`) | Max number of parallel builds (default: 1) |

### Examples

Build a package by path:

```
wright-build /var/hold/extra/nginx
```

Build a package by name (resolved from plan directories):

```
wright-build nginx
```

Build an assembly (group of packages):

```
wright-build @base-system
```

Update checksums for a package:

```
wright-build --update nginx
```

Validate a package description without building:

```
wright-build --lint nginx
```

Build in parallel:

```
wright-build -j 4 @desktop
```

### Build Output

On success, prints the path to the created `.wright.tar.zst` archive. The archive is placed in the components directory (default: `components/` or current directory).

Build logs are stored in the build directory (default: `/tmp/wright-build/<name>-<version>/log/`).

---

## wright-repo

Repository management tool for generating package indexes from built packages.

```
wright-repo <COMMAND>
```

### Commands

#### `wright-repo generate [PATH]`

Generate repository index from a hold tree.

```
wright-repo generate /var/hold --output /var/www/repo
```

**Options:**

| Flag | Description |
|------|-------------|
| `[PATH]` | Path to the hold tree (root containing core/base/extra). Defaults to `.`. |
| `--output <DIR>` | Output directory for the index (default: `<PATH>/packages`). |

Scans the hold tree for `package.toml` files and checks for corresponding binary packages in the output directory. It verifies the existence and integrity of packages before adding them to the index.
