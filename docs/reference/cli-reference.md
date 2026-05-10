# CLI Reference

Wright provides one CLI, `wright`, with top-level subcommands for both
build-side and system-side workflows.

## Global Options

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Load configuration from this file instead of the default search path |
| `--db <PATH>` | Override the system database path |
| `--root <PATH>` | Override the target root directory |
| `-v`, `-vv` | Increase log verbosity (info / debug) |
| `--quiet` | Suppress all output except errors |

## System Commands

### `wright install <TARGET...>`

Installs outputs produced by plan names or plan directories. Wright reads the
plan manifests and installs the expected archives from `parts_dir`. Runtime
dependency problems are reported as warnings, not errors.

| Flag | Description |
|------|-------------|
| `--force` | Reinstall even if the part is already installed and up to date |
| `--nodeps` | Suppress runtime dependency warnings |
| `--path` | Treat arguments and stdin as explicit archive paths |

### `wright upgrade <PART...>`

Upgrades installed parts by archive path or by the latest matching archive in `parts_dir`.

| Flag | Description |
|------|-------------|
| `--force` | Upgrade even if the incoming version is not newer |

### `wright sysupgrade`

Upgrades every installed part to its newest matching archive in `parts_dir`.

### `wright remove <PART...>`

Removes installed parts and optionally cleans up orphaned dependencies.

| Flag | Description |
|------|-------------|
| `--cascade` | Also remove `dependency`-origin parts that become unreferenced |
| `--force` | Remove even if other parts depend on this one |
| `--plan <NAME>` | Remove all parts produced by the named plan |

### `wright apply <TARGET...>`

Resolves plans, plan directories, or group names (prefixed with `@`), checks
archives in `parts_dir`, automatically adds missing or outdated dependency
plans, builds each wave, and then installs or upgrades each wave before
continuing.

| Flag | Description |
|------|-------------|
| `--deps [link\|runtime\|build\|all]` | Dependency domain to expand |
| `--rdeps [link\|runtime\|build\|all]` | Reverse dependency domain to expand |
| `--match [missing\|outdated\|installed\|all]` | Which dependency state triggers inclusion |
| `--depth <N>` | Maximum expansion depth |
| `-f`, `--force` | Force rebuild and reinstall |
| `--fresh` | Discard prior workflow state and start from scratch |
| `--dry-run` | Print the plan without executing it |

### `wright list`

Lists installed parts.

| Flag | Description |
|------|-------------|
| `-l`, `--long` | Show origin, version, release, arch, and plan columns |
| `--roots` | Show only parts that nothing else depends on |
| `--orphans` | Show `dependency`-origin parts no longer needed by anything |
| `--assumed` | Show `external`-origin parts registered via `wright assume` |
| `--plan <NAME>` | Show only parts produced by the named plan |

### `wright query <PART>`

Shows dependency tree and reverse-dependency tree for an installed part.

### `wright search <KEYWORD>`

Searches part names and descriptions for the given keyword.

### `wright files <PART>`

Lists all files owned by an installed part.

### `wright owner <FILE>`

Shows which installed part owns the given file path.

### `wright verify [PART]`

Verifies file checksums against recorded hashes. Checks all parts when `PART` is omitted.

### `wright doctor`

Runs a series of integrity checks on the installed state and reports problems.

### `wright history [PART]`

Shows transaction history. Filters to the named part when specified.

### `wright assume <NAME> <VERSION>`

Registers a part as externally provided — it is known to be present on the system
but was not installed by Wright (e.g. host toolchain tools during LFS bootstrap).
Assumed parts have no filesystem footprint; they only satisfy dependency checks
and `wright resolve` queries.

```bash
wright assume gcc 14.2.0
wright assume --file assumed-parts.txt
echo "glibc 2.40" | wright assume
```

| Flag | Description |
|------|-------------|
| `--file <FILE>` | Read `name version` lines from a file |

### `wright unassume <NAME>`

Removes an assumed (`external`-origin) part record.

### `wright mark <PART...>`

Changes the recorded origin of one or more installed parts.

| Flag | Description |
|------|-------------|
| `--as-manual` | Mark as `manual` (user-requested; not auto-removable) |
| `--as-dependency` | Mark as `dependency` (eligible for orphan cleanup) |

### `wright launch`

Converges a target root from a group manifest or from explicit plan names.
Before building, `launch` prepares the target root with a complete Wright
infrastructure:

1. Creates the directory skeleton (`var/lib/wright/`, `var/log/wright/`,
   `etc/wright/`, ...).
2. Copies all source plans into `<root>/var/lib/wright/plans/` so the target
   can self-maintain later.
3. Copies referenced group manifests into `<root>/var/lib/wright/groups/`.
4. Writes a minimal `/etc/wright/wright.toml` pointing at the target-local
   directories.
5. Initialises the SQLite database with the full schema.

It then resolves dependencies, builds each wave, packages outputs, installs
them into `--root`, and applies declarative `[config]` (hostname, timezone,
locale, services).  Build and package outputs are isolated under the target
root so the host system is not polluted.

```bash
wright launch --root /mnt/new --group ./groups/core.toml
wright launch --root /mnt/new --plans ./plans bash coreutils glibc
wright launch --root /mnt/new --plans ./plans @core
```

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Required. The target root to fill. |
| `--group <FILE>` | Path to a `group.toml` manifest naming the plans to build and install. |
| `--plans <DIR>` | Source path: take plans from this directory. Positional arguments are plan names or `@group` references. |
| `--dry-run` | Print install order and config actions without writing anything. |
| `--force` | Rebuild and reinstall parts that are already present in the target. |

Re-running `launch` on the same root converges drift rather than erroring.

---

## Build Commands

### `wright build <TARGET...>`

Builds plans into staging and output directories under `build_dir`.

| Flag | Description |
|------|-------------|
| `--force` | Rebuild even if staged outputs already exist; force re-run of all lifecycle stages even when stage sentinels exist |
| `--clean` | Remove the build workspace before building |
| `--fresh` | Discard prior workflow state and start from scratch |
| `--stage <NAME>` | Start execution at this stage (skip earlier stages) |
| `--until-stage <NAME>` | Stop after this stage |
| `--skip-check` | Skip the `check` lifecycle stage |
| `--mvp` | Run the MVP (bootstrap) build pass only |
| `--fetch` | Only run the `fetch` and `verify` stages |
| `--checksum` | Verify source checksums only |

### `wright package <TARGET...>`

Slices existing staging directories into output archives and writes them to `parts_dir`.

| Flag | Description |
|------|-------------|
| `--force` | Re-slice outputs from staging and overwrite existing archives |
| `--out-dir <PATH>` | Write produced archives to this directory instead of `parts_dir` |
| `--print-parts` | Print produced archive paths on stdout |

### `wright resolve <TARGET...>`

Expands the dependency and rebuild scope without building. Prints a newline-separated list
of plan names suitable for piping into `wright build`.

| Flag | Description |
|------|-------------|
| `--exclude-targets` | Exclude the listed targets from output (print only the expanded set) |
| `--deps [link\|runtime\|build\|all]` | Dependency domain to expand upward |
| `--rdeps [link\|runtime\|build\|all]` | Reverse dependency domain to expand downward |
| `--match [missing\|outdated\|installed\|all]` | Which state triggers inclusion |
| `--depth <N>` | Maximum expansion depth |
| `--tree` | Print a tree view instead of a flat list |
| `--installed` | Only include plans with at least one installed part |

### `wright prune`

Removes stale archives from `parts_dir`.

| Flag | Description |
|------|-------------|
| `--latest` | Keep only the most recent archive for each part name |
| `--apply` | Actually delete; dry-run by default |
| `--untracked` | Also remove archives not referenced by any local plan |

---

## Lint Command

### `wright lint [TARGET...]`

Validates plan syntax, dependency reference format, local plan and output
references, and dependency graph cycles. When no targets are specified,
lints all plans found under `plans_dir`.

| Flag | Description |
|------|-------------|
| `-r`, `--recursive` | Recurse into subdirectories when scanning for plans |

---

## Common Pipelines

Build a part and install it:

```bash
wright build curl
wright package curl
wright install curl
```

Let `apply` drive the whole source-first workflow:

```bash
wright apply curl
wright apply @core
wright apply @core openssl
```

Rebuild all ABI-sensitive reverse dependents and install:

```bash
wright resolve zlib --rdeps=all --depth=0 > /tmp/wright-rebuild
wright build --force $(cat /tmp/wright-rebuild)
wright package --print-parts $(cat /tmp/wright-rebuild) | wright install --path
```

Mark a previously auto-installed part as user-requested:

```bash
wright mark openssl --as-manual
```

Register host-provided parts during LFS bootstrap:

```bash
wright assume gcc 14.2.0
wright assume glibc 2.40
```
