# CLI Reference

Wright provides a single unified `wright` binary. All functionality is accessed
through subcommands, organised into four groups that match the reader's intent:

- **System Management** — mutate live system state
- **Query & Inspection** — read-only introspection
- **Build & Packaging** — forge, lint, and bootstrap workflows
- **Cache & Maintenance** — house-keeping and cleanup

## Global Options

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Load configuration from this file instead of the default search path |
| `--db <PATH>` | Override the system database path |
| `--root <PATH>` | Override the target root directory |
| `-v`, `-vv` | Increase log verbosity (info / debug) |
| `--quiet` | Suppress all output except errors |

## System Management

### `wright merge <TARGET...>`

Merge part archives into the target root. By default, arguments are plan names
or plan directories. Wright reads each plan manifest, derives the expected
output archive names, and merges those archives from `parts_dir`. Use `--path`
to merge explicit archive paths instead.

| Flag | Description |
|------|-------------|
| `--force` | Force redeploy even if already deployed |
| `--nodeps` | Skip runtime dependency warnings |
| `--path` | Treat arguments and stdin as explicit archive paths |

### `wright install <TARGET...>`

Install plans to the local system with full lifecycle (resolve → forge → seal → deploy).
Targets may be plan names, plan directories, or folio names prefixed with `@`.
Automatically pulls in missing or outdated dependencies under the selected
match policy.

```bash
wright install zlib
wright install zlib openssl
wright install @core
wright install gcc --match=all
```

| Flag | Description |
|------|-------------|
| `-d`, `--deps [link\|runtime\|build\|all]` | Dependency domain to expand |
| `-r`, `--rdeps [link\|runtime\|build\|all]` | Reverse dependency domain to expand |
| `--match [missing\|outdated\|installed\|all]` | Which dependency state triggers inclusion |
| `--depth <N>` | Maximum expansion depth |
| `-f`, `--force` | Force reforge and redeploy |
| `-n`, `--dry-run` | Print the plan without executing it |

### `wright upgrade <TARGET...>`

Upgrade plans to the latest version. When given plan names, checks if the plan
has a newer version than what is deployed, then resolves, forges, seals, and deploys it
along with reverse link dependencies (for ABI consistency). Use `all` to check
every installed plan.

```bash
wright upgrade zlib
wright upgrade all
wright upgrade zlib --force
```

| Flag | Description |
|------|-------------|
| `-f`, `--force` | Force reforge and redeploy even if the plan version matches |
| `--depth <N>` | Maximum depth for reverse dependency expansion |

### `wright remove <PART...>`

Remove deployed parts by name. Removal is blocked when another deployed part
depends on the target unless `--recursive` or `--force` is used.

```bash
wright remove zlib
wright remove zlib --recursive
wright remove zlib --cascade
```

| Flag | Description |
|------|-------------|
| `--force` | Force removal even if other parts depend on this one |
| `-r`, `--recursive` | Recursively remove all parts that depend on the target |
| `-c`, `--cascade` | Also remove orphan dependencies (auto-deployed deps) |

### `wright assume <NAME> <VERSION>`

Mark a part as externally provided so dependency checks consider it satisfied.
Assumed parts have no filesystem footprint; they only satisfy dependency checks.

```bash
wright assume gcc 14.2.0
wright assume --file assumed-parts.txt
echo "glibc 2.40" | wright assume
```

| Flag | Description |
|------|-------------|
| `--file <FILE>` | Read `name version` pairs from a file |

### `wright unassume <NAME>`

Remove an assumed (`external`-origin) part record created with `wright assume`.

## Query & Inspection

### `wright list`

List deployed parts.

```bash
wright list
wright list -l
wright list --roots
wright list --orphans
wright list --assumed
```

| Flag | Description |
|------|-------------|
| `-l`, `--long` | Show origin, version, release, and architecture |
| `--roots` | Show only top-level (root) parts with no deployed dependents |
| `--orphans` | Show orphan parts (auto-deployed deps no longer needed) |
| `--assumed` | Show assumed (externally provided) parts |

### `wright files <PART>`

List files owned by a deployed part.

### `wright check [PART]`

Perform system health checks covering database integrity, file conflicts,
shadowed files, and runtime dependency resolution. With `--deep`, walk each
deployed part's ELF binaries and verify their `DT_NEEDED` entries.

| Flag | Description |
|------|-------------|
| `--deep` | Walk ELF binaries and verify `DT_NEEDED` entries |
| `--integrity-only` | Only run integrity checks (database, file conflicts, shadows) |

### `wright history [PART]`

Show part transaction history (deploy, upgrade, remove). Filters to the named
part when specified.

### `wright doctor`

Run comprehensive system health checks: database integrity, file conflicts,
registry dependency resolution, ELF `DT_NEEDED` verification, and a global
`parts_dir` dependency closure scan. Use after batch deployments to detect
missing providers and stale dependencies.

## Build & Packaging

### `wright build <TARGET...>`

Build (forge) plans into staging and output directories under `build_dir`.

```bash
wright build zlib
wright build zlib --rebuild --clean
wright build freetype --until-stage=staging
```

| Flag | Description |
|------|-------------|
| `-c`, `--clean` | Clear the forge workspace before building |
| `-R`, `--rebuild` | Reforge from scratch: bypass stage checkpoints and re-run all lifecycle stages |
| `--stage <NAME>` | Run only the specified lifecycle stages; may be repeated |
| `--force-stage <NAME>` | Force re-run of a specific stage even if its checkpoint is valid |
| `--until-stage <NAME>` | Run a normal forge pipeline and stop after the specified stage |
| `--skip-check` | Skip the lifecycle `check` stage |
| `--mvp` | Forge using the MVP dependency set from mvp.toml |
| `--fetch` | Download sources only; do not forge |
| `--checksum` | Compute and update SHA256 checksums in plan.toml |

### `wright lint [TARGET...]`

Validate plan syntax, dependency reference format, local plan and output
references, and dependency graph cycles. When no targets are specified,
lints all plans found under `plans_dir`.

| Flag | Description |
|------|-------------|
| `-r`, `--recursive` | Recurse into subdirectories when scanning for plans |
| `--verify` | Verify deployed part file integrity (SHA-256 checksums) |

### `wright launch`

Fill a target root from a folio manifest or from explicit plan names. Before
forging, `launch` prepares the target root with a complete Wright
infrastructure: directory skeleton, synced plans and folios, a minimal
`wright.toml`, and an initialised database. Re-running `launch` on the same
root converges drift rather than erroring.

```bash
wright launch --root /mnt/new --folio ./folios/core.toml
wright launch --root /mnt/new --plans ./plans bash coreutils glibc
wright launch --root /mnt/new --plans ./plans @core
```

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Required. The target root to fill. |
| `--folio <FILE>` | Path to a `folio.toml` manifest naming the plans to forge and deploy. |
| `--plans <DIR>` | Source path: take plans from this directory. Positional arguments are plan names or `@folio` references. |
| `-n`, `--dry-run` | Print deploy order and config actions without writing anything. |
| `-f`, `--force` | Reforge and redeploy parts that are already present in the target. |

## Cache & Maintenance

### `wright prune`

Remove stale archives from `parts_dir`.

| Flag | Description |
|------|-------------|
| `--latest` | Keep only the most recent archive for each part name |
| `--apply` | Apply deletions; dry-run by default |

## Common Pipelines

Forge a part and deploy it:

```bash
wright build zlib
wright install zlib
```

Install with automatic dependency resolution:

```bash
wright install curl
wright install @core
wright install @core openssl
```

Register host-provided parts during LFS bootstrap:

```bash
wright assume gcc 14.2.0
wright assume glibc 2.40
```

## Porcelain vs Plumbing

Wright commands fall into two layers:

- **Porcelain** — user-facing commands that are safe to run interactively and
  produce human-readable output. Examples: `install`, `upgrade`, `doctor`,
  `check`, `launch`.
- **Plumbing** — low-level primitives intended for scripting, piping, and CI.
  They do one thing, produce machine-parseable output by default, and carry
  fewer guard-rails. Examples: `merge` (direct archive deployment),
  `build` (compile without auto-deploy), `list` (plain newline-separated
  names for `xargs`).

This distinction is advisory; no command is artificially restricted from
interactive use or scripting.
