# CLI Reference

Wright provides a single unified `wright` binary. All functionality is accessed
through subcommands, organized into four groups that match the reader's intent:

- **System Management** — mutate live system state
- **Query & Inspection** — read-only introspection
- **Build & Packaging** — forge, lint, and bootstrap workflows
- **Cache & Maintenance** — housekeeping and cleanup

## Global Options

| Flag | Description |
|------|-------------|
| `--config <PATH>` | Load configuration from this file instead of the default search path |
| `--db <PATH>` | Override the system database path |
| `-v`, `-vv` | Increase log verbosity (`-v` = debug, `-vv` = trace; default is info) |
| `--quiet` | Reduce log output to warnings and errors only |

All four global options may appear before or after the subcommand. `-v` and
`--quiet` conflict. Commands that operate on a target filesystem expose their
own `--root <PATH>` option.

Short flags are allocated globally, so the same letter means the same thing on
every command: `-f` = `--force`, `-n` = `--dry-run`, `-c` = `--clean`,
`-d` = `--deps`, `-r` = `--rdeps`, `-t` = `--tree`, `-l` = `--long`,
`-o` = `--orphans`, `-p` = `--print-parts`.

## Errors and Exit Status

A failed command prints a multi-line failure report on stderr and exits with
status 1; no command exits mid-operation. Two commands keep their own machine
contracts:

- `wright check` exits 0 when everything resolves and 1 when any check fails,
  so it is suitable for CI gates.
- `wright owner` exits 1 when any given path is unowned; with `--json` the
  JSON array is printed first and the error report goes to stderr.

Interrupting a command with SIGINT exits with status 130.

## System Management

### `wright merge <TARGET...>`

Merge part archives into the target root. By default, arguments are plan names
or plan directories. Wright reads each plan manifest, derives the expected
output archive names, and merges those archives from `parts_dir`. Use `--path`
to merge explicit archive paths instead.

| Flag | Description |
|------|-------------|
| `-f`, `--force` | Force redeploy even if already deployed |
| `-n`, `--dry-run` | Print `[dry-run] merge -> <root>` and the resolved archive list without changing anything |
| `--nodeps` | Skip runtime dependency warnings |
| `--path` | Treat arguments and stdin as explicit archive paths |
| `--root <PATH>` | Operate on this target root instead of `/` |

### `wright install <TARGET...>`

Install plans to the local system with the full lifecycle
(`resolve → build → package → merge`; `seal` is an alias of `package`).
Targets may be plan names, plan directories, or folio names prefixed with `@`.
Automatically pulls in missing or outdated dependencies under the selected
match policy.

```bash
wright install zlib
wright install zlib openssl
wright install zlib --clean
wright install @core
wright install gcc --match=all
```

| Flag | Description |
|------|-------------|
| `-d`, `--deps [link\|runtime\|build\|all]` | Forward dependency domain to expand; a bare `--deps` means `all`, and omitting the flag follows all domains |
| `-r`, `--rdeps [link\|runtime\|build\|all]` | Additionally rebuild deployed reverse dependents; a bare `--rdeps` means `link`, and omitting the flag skips reverse expansion |
| `--match <missing\|outdated\|installed\|all>` | Which dependency state triggers inclusion; requires a value, may be repeated, and defaults to `outdated` |
| `--depth <N>` | Maximum expansion depth |
| `-c`, `--clean` | Clear forge state before building plans that need an update; does not redeploy up-to-date plans |
| `-f`, `--force` | Cleanly reforge and redeploy, including up-to-date plans |
| `-n`, `--dry-run` | Fully resolve the wave plan and print it (`[dry-run] install -> <root>`, then one `batch N:` line per batch) without forging or deploying |
| `--root <PATH>` | Operate on this target root instead of `/` |

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
| `-n`, `--dry-run` | Resolve the upgrade set (including reverse-dependency expansion) and print it without building anything |
| `--depth <N>` | Maximum depth for reverse dependency expansion |
| `--root <PATH>` | Operate on this target root instead of `/` |

### `wright remove <TARGET...>`

Remove deployed parts. Each target follows the universal plan/output
identifier scheme:

| Form | Addresses |
|------|-----------|
| `plan` | Every deployed output of the plan |
| `plan:*` | Every deployed output of the plan (absolute form) |
| `output` | The single deployed output |
| `plan:output` | The named output of the plan (absolute form) |

A bare name that matches both a plan with deployed outputs and an output is
ambiguous and rejected; the error names the absolute forms to use. A
single-output plan whose output carries the plan name resolves without
ambiguity. `plan:output` membership is validated: naming the wrong plan is
an error.

Removal is blocked when another deployed part depends on the target unless
`--recursive` or `--force` is used.

```bash
wright remove zlib
wright remove llvm:clang
wright remove llvm:*
wright remove zlib --recursive
wright remove zlib --cascade
```

| Flag | Description |
|------|-------------|
| `-f`, `--force` | Force removal even if other parts depend on this one |
| `-n`, `--dry-run` | Print the ordered removal plan (recursive dependents first, cascade orphans included) without starting any transaction |
| `--recursive` | Recursively remove all parts that depend on the target |
| `--cascade` | Also remove orphan dependencies (auto-deployed deps) |
| `--root <PATH>` | Operate on this target root instead of `/` |

### `wright provide [PART] [VERSION]`

Mark a part as externally provided so dependency checks consider it satisfied.
Provided parts have no filesystem footprint; they only satisfy dependency checks.

```bash
wright provide gcc 14.2.0
wright provide --file provided-parts.txt
echo "glibc 2.40" | wright provide
```

| Flag | Description |
|------|-------------|
| `--file <FILE>` | Read `name version` pairs from a file |
| `--root <PATH>` | Record the provided part in this target root's database instead of `/` |

## Query & Inspection

### `wright list`

List deployed parts.

```bash
wright list
wright list -l
wright list --roots
wright list --orphans
wright list --provided
wright list --json
```

| Flag | Description |
|------|-------------|
| `-l`, `--long` | Show origin, version, release, and architecture |
| `--roots` | Show only top-level (root) parts with no deployed dependents |
| `-o`, `--orphans` | Show orphan parts (auto-deployed deps no longer needed) |
| `--provided` | Show provided (externally provided) parts |
| `--json` | Emit a machine-readable JSON array of part records |
| `--root <PATH>` | Query this target root instead of `/` |

### `wright files <TARGET>`

List files owned by deployed parts. The target follows the same universal
plan/output identifier scheme as `wright remove`: `plan` or `plan:*` lists
the files of every deployed output of the plan; `output` or `plan:output`
lists a single output. When a plan target resolves to multiple outputs,
each printed line is prefixed with the owning output name.

| Flag | Description |
|------|-------------|
| `--json` | Emit machine-readable JSON instead of text |
| `--root <PATH>` | Query this target root instead of `/` |

### `wright owner <FILE>...`

Show which deployed part owns each given file path. The inverse of
`wright files`: given a file, return the owning part.

Relative paths are resolved against the current directory; existing paths are
canonicalised (symlinks followed) so that lookups match the path actually
recorded in the database. A non-existent path is looked up as-is, which lets
you query files that were deleted out-of-band but still tracked.

With one argument, the part name is printed on its own line. With multiple
arguments, each result is prefixed with the resolved path (`<file>: <part>`).
If a file is claimed by more than one deployed part (a conflict surfaced by
`wright check`), every owner is printed.

Exits 1 if any of the given paths is not owned by a deployed part; with
`--json` the JSON array is printed first and the error report goes to stderr.

| Flag | Description |
|------|-------------|
| `--json` | Emit machine-readable JSON instead of text |
| `--root <PATH>` | Query this target root instead of `/` |

### `wright check [PART]`

Perform system health checks covering database integrity, file conflicts,
shadowed files, and runtime dependency resolution. With `--deep`, walk each
deployed part's ELF binaries and verify their `DT_NEEDED` entries.

With `--files`, verify every deployed file recorded in the database exists on
disk (and is the correct type: file/symlink/directory). Use this to detect
files deleted by external tools or partially-uninstalled parts.

| Flag | Description |
|------|-------------|
| `--deep` | Walk ELF binaries and verify `DT_NEEDED` entries |
| `--files` | Verify every deployed file exists on disk |
| `--integrity-only` | Only run integrity checks (database, file conflicts, shadows) |
| `--json` | Emit a machine-readable JSON report instead of text |
| `--root <PATH>` | Check this target root instead of `/` |

### `wright history [PART]`

Show part transaction history (deploy, upgrade, remove). Filters to the named
part when specified.

| Flag | Description |
|------|-------------|
| `--json` | Emit machine-readable JSON instead of text |
| `--root <PATH>` | Query this target root instead of `/` |

### `wright doctor`

Run comprehensive system health checks: database integrity, file conflicts,
deployed file existence, registry dependency resolution, ELF `DT_NEEDED`
verification, and a global `parts_dir` dependency closure scan. Use after
batch deployments to detect missing files, providers, and stale dependencies.
Also reports plans whose source changed since their parts were installed
(provenance drift); when a plan-source snapshot was recorded, a unified diff
between the snapshot and the current source follows the report line. Drift is
advisory and never fails the run.

| Flag | Description |
|------|-------------|
| `--root <PATH>` | Diagnose this target root instead of `/` |

### `wright plan <TARGET>`

Print the plan source recorded when the plan's parts were sealed. The output
is the exact `plan.toml` bytes, so it round-trips onto disk even if the plan
file has since been edited or deleted:

```bash
wright plan zlib
wright plan zlib > plan.toml
```

Plans whose parts predate plan-source snapshots (ADR-0033) have none;
rebuild and re-deploy to record one.

| Flag | Description |
|------|-------------|
| `--json` | Emit machine-readable JSON instead of raw plan source |
| `--root <PATH>` | Query this target root instead of `/` |

### JSON Output

`list`, `files`, `owner`, `history`, `plan`, and `check` accept `--json`. Empty
results print `[]` (`check` prints a report object with an empty `issues`
array). Output shapes:

| Command | Shape |
|---------|-------|
| `list --json` | Array of `{"name","version","release","epoch","arch","origin","plan_name"}` |
| `files --json` | `{"part","files":[...]}` for an output target; `{"plan","outputs":[{"part","files":[...]},...]}` for a plan target |
| `owner --json` | Array of `{"path","owners":[...]}` |
| `history --json` | Array of `{"timestamp","session_id","command","part","action","old_version","new_version","status"}` |
| `plan --json` | `{"plan","checksum","source"}` |
| `check --json` | `{"scope","mode","issue_count","issues":[...]}`; each issue carries a `check` tag (e.g. `missing-file`, `broken-dependency`, `unresolved-soname`) |

`check --json` prints the report first and still exits 1 when issues are
found — the exit code remains the primary machine interface.

## Build & Packaging

### `wright resolve <TARGET...>`

Resolve plan names and optionally expand their dependencies or reverse
dependents. The default output is one plan name per line for use in pipelines.
Use `--tree` for a human-readable dependency forest.

```bash
wright resolve hello
wright resolve hello --deps --match=outdated
wright resolve openssl --rdeps=link --depth=0
wright resolve hello --deps --tree
```

| Flag | Description |
|------|-------------|
| `-d`, `--deps [link\|runtime\|build\|all]` | Expand dependencies; an omitted value means `all` |
| `-r`, `--rdeps [link\|runtime\|build\|all]` | Expand reverse dependents; an omitted value means `link` |
| `--match <missing\|outdated\|installed\|all>` | Filter by installed state; may be repeated |
| `--depth <N>` | Limit traversal depth; `0` means unlimited |
| `-t`, `--tree` | Render a dependency forest instead of plain plan names |

### `wright build <TARGET...>`

Build (forge) plans into staging and output directories under `forge_dir`.

```bash
wright build zlib
wright build zlib --force --clean
wright build freetype --until-stage=staging
```

| Flag | Description |
|------|-------------|
| `-c`, `--clean` | Clear the forge workspace before building |
| `-f`, `--force` | Reforge from scratch: bypass stage checkpoints and re-run all pipeline stages |
| `--stage <NAME>` | Run only the specified pipeline stages; may be repeated |
| `--force-stage <NAME>` | Force re-run of a specific stage even if its checkpoint is valid |
| `--until-stage <NAME>` | Run a normal forge pipeline and stop after the specified stage |
| `--skip-check` | Skip the pipeline `check` stage |
| `--mvp` | Forge using the MVP dependency set from mvp.toml |
| `--fetch` | Download sources only; do not forge |
| `--seal` | Seal completed builds into local part archives |
| `--checksum` | Compute and update SHA256 checksums in plan.toml |

### `wright package <TARGET...>`

Slice completed staging trees and seal them as `.wright.tar.zst` archives.
`wright seal` is an alias.

```bash
wright package hello
wright package hello --force
wright package hello --print-parts
```

| Flag | Description |
|------|-------------|
| `-f`, `--force` | Re-slice output directories before packaging |
| `-p`, `--print-parts` | Print generated archive paths to standard output |

### `wright lint [TARGET...]`

Validate plan syntax, dependency reference format, local plan and output
references, and dependency graph cycles. When no targets are specified,
lints all plans found under `plans_dir`.

| Flag | Description |
|------|-------------|
| `--recursive` | Recurse into subdirectories when scanning for plans |
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
| `--root <DIR>` | Required. The target root to fill. |
| `--folio <FILE>` | Path to a single folio manifest naming the plans to forge and deploy. Mutually exclusive with positional targets. |
| `--plans <DIR>` | Source path: take plans from this directory. Positional arguments are plan names or `@folio` references. |
| `--folios <DIR>` | Source path: resolve `@folio` references from this directory. |
| `-n`, `--dry-run` | Print deploy order and config actions without writing anything. |
| `-f`, `--force` | Reforge and redeploy parts that are already present in the target. |

## Cache & Maintenance

### `wright clean [TARGET...]`

Remove build workspaces for selected plans, or all plan workspaces when no
plan is given. Archive and command-log cleanup are explicit options.

| Flag | Description |
|------|-------------|
| `--parts` | Also remove matching local part archives |
| `--logs` | Also remove Wright command logs |

### `wright prune`

Remove older archive versions while retaining the latest version of each part.
Bare `wright prune` is a dry run; pass `--apply` to actually delete.

| Flag | Description |
|------|-------------|
| `--latest` | Keep only the latest archive version of each part (currently the only mode; accepted for forward compatibility) |
| `--apply` | Actually delete the selected archives |

## Common Pipelines

Forge a part and deploy it:

```bash
wright build zlib
wright package zlib
wright merge zlib
```

Install with automatic dependency resolution:

```bash
wright install curl
wright install @core
wright install @core openssl
```

Register host-provided parts during LFS bootstrap:

```bash
wright provide gcc 14.2.0
wright provide glibc 2.40
```

## Porcelain vs Plumbing

Wright commands fall into two layers:

- **Porcelain** — user-facing commands that are safe to run interactively and
  produce human-readable output. Examples: `install`, `upgrade`, `doctor`,
  `check`, `launch`.
- **Plumbing** — low-level primitives intended for scripting, piping, and CI.
  They do one thing, produce machine-parseable output by default, and carry
  fewer guardrails. Examples: `resolve` (plain newline-separated plan names),
  `package` (archive creation), and `merge` (direct archive deployment).

This distinction is advisory; no command is artificially restricted from
interactive use or scripting.
