# Plan Manifest (`plan.toml`)

Reference for every key, table, and field in a Wright plan manifest.

## Top-Level Metadata

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Part name |
| `version` | string | no | — | Free-form version string. Omit for rolling/VCS builds. |
| `release` | integer | yes | — | Build revision (must be >= 1) |
| `epoch` | integer | no | `0` | Version epoch — overrides version comparison when upstream changes versioning scheme |
| `description` | string | yes | — | Short description (must not be empty) |
| `license` | string | yes | — | SPDX license identifier |
| `arch` | string | yes | — | Target architecture (e.g. `x86_64`) |
| `url` | string | no | — | Upstream project URL |
| `maintainer` | string | no | — | Maintainer name and email |

### `epoch`

Forces a part to be considered newer than any version with a lower epoch, regardless of the version string. Used when upstream changes their versioning scheme in a way that makes the new version sort lower (e.g. renaming from `2024.1` to `1.0.0`).

Only set when upstream makes a breaking versioning scheme change. Leave at `0` (or omit) for normal releases.

When non-zero, the archive filename includes it: `name-epoch:version-release-arch.wright.tar.zst`.

### Omitting `version`

When `version` is omitted:

- The field is absent from `.PARTINFO`.
- Archive filenames omit the version segment: `name-release-arch.wright.tar.zst`.
- The build directory uses a `<name>-noversion` suffix.
- Dependency version constraints automatically treat the part as satisfying all constraints.
- `wright list --long` displays `-` in place of the version.

## Plan-Level Dependencies

Declared as top-level fields; affect build planning and `wright resolve`.

| Field | Type | Description |
|-------|------|-------------|
| `build_deps` | list of strings | Build-time dependencies mounted into the isolation environment |
| `link_deps` | list of strings | ABI-sensitive linked dependencies. Triggers rebuild on update |

Constraint operators: `>=`, `<=`, `>`, `<`, `=`.

Dependency references accept two forms:

| Form | Meaning | Example |
|------|---------|---------|
| `plan` | All outputs of `plan` | `openssl` |
| `plan:output` | Exactly one output | `llvm:llvm-libs` |

Version constraints follow the reference:

```toml
link_deps = ["pcre2 >= 10.42"]
```

`wright lint` validates that each referenced local plan exists. For explicit `plan:output` references, it also checks that the output is declared by that plan.

When a plan produces multiple outputs, use `plan:output` to depend on exactly one:

```toml
build_deps = ["llvm:clang", "llvm:lld"]
```

Writing `clang` (bare plan name) would mean "all outputs of the `clang` plan," which is different from "the `clang` output of the `llvm` plan."

## Output-Level Dependencies

Runtime dependencies are declared per-output inside each `[[output]]` entry via `runtime_deps`. There is no plan-level fallback.

## Sources (`[[sources]]`)

Array-of-tables with a mandatory `type` field.

### `type = "http"`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | Remote URL (`http://` or `https://`) |
| `sha256` | string | required | SHA-256 checksum. Use `"SKIP"` only during development or for untrusted sources |
| `as` | string | optional | Rename the downloaded file in the cache |
| `extract_to` | string | optional | Subdirectory under `${WORKDIR}` to extract/copy the file into |

### `type = "git"`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `url` | string | required | Git repository URL |
| `ref` | string | `"HEAD"` | Branch, tag, or commit hash to check out |
| `depth` | integer | optional | Shallow clone depth. Defaults to `1`. Set to `null` or omit for full clone. Disabled for 40-character commit hashes |
| `extract_to` | string | optional | Subdirectory under `${WORKDIR}` to check out into |

### `type = "local"`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | required | Path relative to the plan directory. Must not escape the plan directory |
| `extract_to` | string | optional | Subdirectory under `${WORKDIR}` to copy the file into |

### Archive Handling

- Archives with supported extensions (`.tar.gz`, `.tgz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.tar.lz`, `.zip`) are automatically extracted during the `extract` stage.
- Non-archive files are copied directly to `${WORKDIR}` (or the `extract_to` subdirectory).

## Options (`[options]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `static` | bool | `false` | Build statically linked binaries |
| `debug` | bool | `false` | Build with debug info |
| `ccache` | bool | `true` | Use ccache for compilation if available |
| `env` | map of strings | `{}` | Environment variables injected into every lifecycle stage |
| `memory_limit` | integer | — | Max virtual address space per build process (MB), overrides global |
| `cpu_time_limit` | integer | — | Max CPU time per build process (seconds), overrides global |
| `timeout` | integer | — | Wall-clock timeout per build stage (seconds), overrides global |
| `skip_fhs_check` | bool | `false` | Skip FHS validation after output slicing |

Per-plan values override global (`wright.toml`) settings.

## Lifecycle Stages (`[lifecycle.<stage>]`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `executor` | string | `"shell"` | Executor to run the script with |
| `isolation` | string | `"strict"` | Security isolation level |
| `env` | map of strings | `{}` | Extra environment variables |
| `script` | string | `""` | The script to execute |

The `env` field supports variable substitution in values.

## Lifecycle Order (`[lifecycle_order]`)

Override the default pipeline order:

```toml
[lifecycle_order]
stages = ["fetch", "verify", "extract", "configure", "compile", "staging"]
```

## MVP Overrides (`mvp.toml`)

Allowed top-level fields in `mvp.toml`:

- `build_deps`
- `link_deps`
- `lifecycle`
- `lifecycle_order`

MVP `build_deps` and `link_deps` override the top-level plan fields. Any field omitted falls back to `plan.toml`.

Resolution order for the MVP pass:

1. If `[lifecycle.<stage>]` exists in `mvp.toml`, it is used.
2. Otherwise, it falls back to `[lifecycle.<stage>]` in `plan.toml`.

Do not duplicate part metadata, sources, outputs, or hooks in `mvp.toml`.

## Hooks (`[[output]].hooks`)

Transaction-time scripts that run on the live system.

| Field | Type | Description |
|-------|------|-------------|
| `pre_install` | string | Run before first install |
| `post_install` | string | Run after first install |
| `post_upgrade` | string | Run after upgrade |
| `pre_remove` | string | Run before part removal |
| `post_remove` | string | Run after part removal |

## Output Modes

A plan can use either implicit or explicit output mode.

| Mode | Syntax | Use case |
|------|--------|----------|
| Default (no output section) | Omit `[[output]]` | Simple package; everything in `${STAGING_DIR}` becomes the part named after `plan.name` |
| Explicit outputs | `[[output]]` array-of-tables | Attach output metadata, split files, discard files, or enforce explicit coverage |

`[output]` table syntax is not supported. Use `[[output]]` for single-output
metadata as well as split outputs.

### `[[output]]`

| Field | Required | Notes |
|-------|----------|-------|
| `name` | No | Part name for this output. Omit or set to `""` to use `plan.name` |
| `description` | Yes for non-catch-all | Human-readable description |
| `include` | No | Regex patterns for files to claim. Omit on at most one output to define a catch-all |
| `exclude` | No | Regex patterns to exclude |
| `runtime_deps` | No | Per-output runtime dependencies |
| `hooks.*` | No | Per-output transaction hooks |
| `backup` | No | Per-output backup files |
| `replaces` | No | Per-output replacement relations |
| `conflicts` | No | Per-output conflict relations |
| `provides` | No | Per-output virtual provides |

**Coverage rules:**

- Every staged file must be claimed by one `[[output]]`, matched by `[[discard]]`, or claimed by the optional catch-all.
- Plans with unclaimed staged files fail during output slicing.
- At most one catch-all is allowed.
- `description` is not required for catch-all outputs.

### `[[discard]]` (explicitly ignored files)

Use `[[discard]]` only with `[[output]]` mode. It is an array-of-tables even
when only one discard rule is needed.

| Field | Required | Notes |
|-------|----------|-------|
| `include` | **Yes** | Regex patterns for files to ignore |
| `exclude` | No | Regex patterns to keep out of this discard rule |
| `reason` | **Yes** | Human-readable explanation for ignoring matched files |

### Implicit Slicing Order

1. Non-catch-all outputs are processed in declared order.
2. Files matching `include` (and not matching `exclude`) are hard-linked into the output directory.
3. A file is claimed by the first matching output. Later outputs never see it.
4. Remaining files matching `[[discard]]` are ignored.
5. The optional catch-all keeps whatever remains.
6. Remaining files fail slicing.

### Part Relations

Relations are per-output.

| Relation | Behavior |
|----------|----------|
| `replaces` | On install, silently removes any installed part in this list. One-way. Use for renames/merges. |
| `conflicts` | Mutual exclusion. Install refused while a conflicting part is present. Bidirectional. |
| `provides` | Virtual names. Multiple parts can provide the same name. Satisfies dependencies on abstract capabilities. |

### Backup Files

Files listed in `backup` are treated as user-owned config files:

- **On upgrade:** the new default is written as `<path>.wnew`. The live file is left intact.
- **On remove:** config files are not deleted.

## Default Lifecycle Pipeline

| Stage | Type | Description |
|-------|------|-------------|
| `fetch` | built-in | Download sources and copy local files |
| `verify` | built-in | Verify SHA-256 checksums |
| `extract` | built-in | Extract archives, copy non-archives to `${WORKDIR}` |
| `prepare` | user | Pre-build setup (e.g. apply patches) |
| `configure` | user | Run configure scripts |
| `compile` | user | Compile the software |
| `check` | user | Run test suites |
| `staging` | user | Install files into `${STAGING_DIR}` |

Built-in stages are handled automatically. User stages are only run if defined.

Override with `[lifecycle_order]`.

## Pre/Post Hooks

Any stage can have `pre_<stage>` or `post_<stage>` tables under `lifecycle`. Execution order: `pre_<stage>` → `<stage>` → `post_<stage>`. They support the same fields as any lifecycle stage.

## Variable Substitution

Variables use `${VAR_NAME}` syntax and are expanded in scripts and source URIs. Unrecognized variables are left as-is.

| Variable | Description |
|----------|-------------|
| `${NAME}` | Current output name |
| `${VERSION}` | Version from `version` (absent if omitted) |
| `${RELEASE}` | Release number as a string |
| `${ARCH}` | Target architecture |
| `${WORKDIR}` | Extraction root directory |
| `${STAGING_DIR}` | Current output staging directory |
| `${MAIN_PART_NAME}` | Primary output name from the top-level `name` field |
| `${MAIN_STAGING_DIR}` | Primary output staging directory |
| `${WRIGHT_BUILD_PHASE}` | Current phase name (`full` or `mvp`) |
| `${WRIGHT_BOOTSTRAP_WITHOUT_<DEP>}` | Set to `1` for each dep excluded in the MVP pass |

## Path Variables

| Variable | Host value (Default) | Isolation value | Description |
|----------|----------------------|-----------------|-------------|
| `${WORKDIR}` | `/var/tmp/wright/workshop/<name>-<version>/work`¹ | `/build` | Root container for all sources |
| `${STAGING_DIR}` | `/var/tmp/wright/workshop/<name>-<version>/staging`¹ | `/output` | Installation target directory (DESTDIR) |

¹ When `version` is omitted, the directory uses `<name>-noversion`.

Inside isolation:
- `/build` is a read-write mount of the host's build work directory.
- `/output` is a read-write mount of the host `staging/` directory for build products.

After lifecycle stages complete, Wright slices `staging/` into `outputs/default/`
and `outputs/<name>/` directories according to `[[output]]` rules. Slicing uses
hard links, so `staging/` remains available for inspection.

## Isolation Levels

| Level | Description |
|-------|-------------|
| `none` | No isolation. Runs directly on the host. |
| `relaxed` | Mount, PID, and UTS namespaces. Network and IPC shared with host. |
| `strict` (default) | Everything in `relaxed` plus network and IPC namespaces. |

In `relaxed` and `strict` modes, the isolation pivots to a minimal root filesystem.  In `strict` mode, Wright mounts a pre-copied read-only sysroot as an overlayfs lower layer with per-task writable upper layers.  Both modes bind-mount `/build` and `/output` read-write, provide `/dev` with basic devices, mount fresh `/proc` and `/tmp`, and set hostname to `wright-isolation`.

If the kernel does not support the required namespaces, falls back to direct execution with a warning.

## Executors

The `executor` field on a lifecycle stage selects which executor to use.

### Built-in: `shell`

Runs scripts with `/bin/bash -e -o pipefail`. Scripts are written to a temporary `.sh` file and passed as an argument to bash.

### Custom Executors

Installed as TOML files in the executor directory:

```toml
[executor]
name = "python"
description = "Python script executor"
command = "/usr/bin/python3"
args = []
delivery = "tempfile"
tempfile_extension = ".py"
required_paths = ["/usr/lib/python3"]
default_isolation = "strict"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Executor name used in lifecycle stages |
| `description` | string | `""` | Human-readable description |
| `command` | string | required | Path to the interpreter |
| `args` | list of strings | `[]` | Arguments before the script path |
| `delivery` | string | `"tempfile"` | How the script is passed to the command |
| `tempfile_extension` | string | `".sh"` | File extension for the temp script |
| `required_paths` | list of strings | `[]` | Extra paths to bind-mount in isolation |
| `default_isolation` | string | `""` | Default isolation level for this executor |

## Validation Rules

| Rule | Detail |
|------|--------|
| `name` | Must match `[a-z0-9][a-z0-9_+.-]*`, max 64 characters |
| `version` | Optional. If present, must be a non-empty alphanumeric string |
| `release` | Must be >= 1 |
| `epoch` | Must be >= 0 (default 0) |
| `description` | Must not be empty |
| `license` | Must not be empty |
| `arch` | Must not be empty |
| `sha256` | Each `[[sources]]` entry has its own `sha256` (use `"SKIP"` for local paths and git sources) |

## Archive Filename Format

- With version: `{name}-{version}-{release}-{arch}.wright.tar.zst`
- Without version: `{name}-{release}-{arch}.wright.tar.zst`
- With epoch > 0: `{name}-{epoch}:{version}-{release}-{arch}.wright.tar.zst` (or `{name}-{epoch}:{release}-{arch}.wright.tar.zst` if version is absent)
