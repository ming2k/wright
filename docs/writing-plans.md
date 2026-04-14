# Writing Plans

A **plan** is a directory containing a `plan.toml` file that describes how to fetch, build, and produce a **part** from a piece of software. This guide is the complete reference for plan authors.

## Directory Structure

Plans live in a flat directory tree. Each plan is a directory named after the part:

```
plans/
├── hello/
│  └── plan.toml
├── nginx/
│  ├── plan.toml
│  ├── mvp.toml
│  └── patches/
│    └── fix-headers.patch
└── python/
  ├── plan.toml
  └── patches/
    ├── 001-fix-paths.patch
    └── 002-no-rpath.patch
```

The directory name should match the `name` field in `plan.toml`. Local files referenced in `[[sources]]` URIs are relative to the plan directory and must not escape it.

If a plan needs a separate bootstrap/MVP override, place it in a sibling
`mvp.toml`. The base file remains `plan.toml`; do not rename it to
`main.toml` or `base.toml`.

## `plan.toml` Reference

### Top-Level Metadata

| Field     | Type   | Required | Default | Description            |
|---------------|----------|----------|---------|------------------------------------|
| `name`    | string  | yes   | —    | Part name             |
| `version`   | string  | yes   | —    | Upstream version (free-form)    |
| `release`   | integer | yes   | —    | Build revision (must be >= 1)   |
| `epoch`    | integer | no    | `0`   | Version epoch — overrides version comparison (see below) |
| `description` | string  | yes   | —    | Short description (must not be empty) |
| `license`   | string  | yes   | —    | SPDX license identifier      |
| `arch`    | string  | yes   | —    | Target architecture (e.g. `x86_64`) |
| `url`     | string  | no    | —    | Upstream project URL        |
| `maintainer` | string  | no    | —    | Maintainer name and email     |

#### Epoch

The `epoch` field forces a part to be considered newer than any version with a lower epoch, regardless of the version string. This is needed when upstream changes their versioning scheme in a way that makes the new version sort lower (e.g. a rename from `2024.1` to `1.0.0`).

```toml
name = "example"
version = "1.0.0"
release = 1
description = "Example part"
license = "MIT"
arch = "x86_64"
epoch = 1    # This will upgrade over any epoch=0 part, even "9999.0"
```

Epoch defaults to `0` and is omitted from archive filenames and `.PARTINFO` when zero. When non-zero, the archive filename includes it: `name-epoch:version-release-arch.wright.tar.zst`.

### `[dependencies]`

All fields default to empty lists if omitted.

| Field    | Type              | Description             |
|-------------|---------------------------------|--------------------------------------|
| `runtime`  | list of strings         | Must be installed at runtime (e.g. bash, python) |
| `build`   | list of strings         | Required only during build (e.g. gcc, cmake) |
| `link`   | list of strings         | ABI-sensitive linked dependencies. Triggers rebuild on update. |
| `optional` | list of strings         | Optional runtime dependencies    |

#### `link` dependencies vs `runtime`

- **`link`**: Use this for ABI-sensitive edges that should trigger rebuilds when the dependency changes. This is a `wright resolve` concept, not an implicit install-time dependency.
- **`runtime`**: Use this for anything that must exist after installation for the part to work.

These lists may overlap, and overlap is often correct for shared libraries. If a library is both linked and required at runtime, declare it in both `link` and `runtime`.

`wright resolve` uses `link` for reverse rebuild expansion. `wright install` uses `runtime` from `.PARTINFO`. Do not rely on `link` alone to pull in runtime requirements.

#### Version constraints

Runtime, build, link entries can include a version constraint:

```toml
link = ["openssl >= 3.0"]
runtime = ["python >= 3.10"]
```

Supported operators: `>=`, `<=`, `>`, `<`, `=`.

#### Optional dependencies

Optional dependencies are simple string lists, like other dependency types:

```toml
optional = ["geoip", "nghttp2"]
```

### `[[sources]]`

Sources use TOML's array-of-tables syntax. Each `[[sources]]` entry declares a single source with its URI and checksum:

| Field   | Type  | Default | Description               |
|-----------|--------|----------|------------------------------------------|
| `uri`   | string | required | Source URI — remote URL (`http://`/`https://`/`git+https://`) or local path relative to the plan directory |
| `sha256` | string | `"SKIP"` | SHA-256 checksum. Use `"SKIP"` for local files or git sources. |

URIs support variable substitution (see [Variable Substitution](#variable-substitution)):

```toml
[[sources]]
uri = "https://nginx.org/download/nginx-${VERSION}.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"

[[sources]]
uri = "patches/fix-headers.patch"
sha256 = "SKIP"
```

#### URI classification

- **Remote URIs** (starting with `http://` or `https://`) are downloaded to the source cache.
- **Git URIs** (starting with `git+https://`) clone a git repository. Use a fragment to specify a branch or tag: `git+https://github.com/foo/bar.git#v1.0`. Always use `sha256 = "SKIP"` for git sources.
- **Local URIs** (everything else) are resolved relative to the plan directory. They must not escape the plan directory (path traversal is blocked).

#### Archive vs non-archive URIs

- URIs pointing to archive files (`.tar.gz`, `.tgz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.tar.lz`, `.zip`) are extracted to the source directory during the `extract` stage.
- Non-archive URIs (patches, config files, scripts, etc.) are copied to `${FILES_DIR}` where lifecycle scripts can access them.

#### Git sources

Clone a specific tag or branch from a git repository:

```toml
[[sources]]
uri = "git+https://github.com/example/repo.git#v1.2.3"
sha256 = "SKIP"
```

The fragment after `#` specifies the branch, tag, or commit to check out. Git sources are always cloned fresh and extracted like archives.

#### Applying patches

Patches are **not** auto-applied. Include them as `[[sources]]` entries and apply them manually in a lifecycle stage. This gives full control over strip level, ordering, and conditions:

```toml
[[sources]]
uri = "https://example.com/foo-${VERSION}.tar.gz"
sha256 = "abc123..."

[[sources]]
uri = "patches/fix-headers.patch"
sha256 = "SKIP"

[[sources]]
uri = "patches/add-feature.patch"
sha256 = "SKIP"

[lifecycle.prepare]
script = """
cd ${BUILD_DIR}
patch -Np1 < ${FILES_DIR}/fix-headers.patch
patch -Np1 < ${FILES_DIR}/add-feature.patch
"""
```

For patches that need a different strip level:

```toml
[lifecycle.prepare]
script = """
cd ${BUILD_DIR}
patch -Np0 < ${FILES_DIR}/special-fix.patch
patch -Np1 < ${FILES_DIR}/normal-fix.patch
"""
```

### `[options]`

| Field        | Type      | Default | Description               |
|---------------------|-----------------|---------|------------------------------------------|
| `static`      | bool      | `false` | Build statically linked binaries     |
| `debug`       | bool      | `false` | Build with debug info          |
| `ccache`      | bool      | `true` | Use ccache for compilation if available |
| `env`        | map of strings | `{}`  | Environment variables injected into every lifecycle stage |
| `memory_limit`   | integer     | —    | Max virtual address space per build process (MB), overrides global |
| `cpu_time_limit`  | integer     | —    | Max CPU time per build process (seconds), overrides global |
| `timeout`      | integer     | —    | Wall-clock timeout per build stage (seconds), overrides global |
| `skip_fhs_check`  | bool      | `false` | Skip FHS validation after the final output stage (`fabricate`). Use only for parts with a deliberate reason to install outside standard paths (e.g. kernel modules). |

Per-plan values override global (`wright.toml`) settings. `memory_limit` and `cpu_time_limit` are enforced via `setrlimit()` before `exec` and inherited by child processes. The wall-clock `timeout` is enforced by the parent process — it catches builds stuck on I/O or deadlocks where CPU time does not advance.

**CPU parallelism:** Wright pins each dockyard process to its computed CPU share via `sched_setaffinity`, so `nproc` inside the dockyard already returns the correct count. Scripts should call `make -j$(nproc)` directly. To override parallelism for a specific part, set `MAKEFLAGS` (or the relevant tool variable) in `[options.env]`. See [resource-allocation.md](resource-allocation.md) for details.

**Practical guidance:** `timeout` is the most important safety net. `memory_limit` limits virtual address space (`RLIMIT_AS`), not physical RSS — set it generously (2-3x expected usage), as programs like rustc, JVM, and Go reserve large virtual mappings they never touch.

### `[lifecycle.<stage>]`

Each lifecycle stage is a TOML table under `lifecycle`:

```toml
[lifecycle.compile]
executor = "shell"
dockyard = "strict"
script = """
cd ${BUILD_DIR}
make -j$(nproc)
"""
```

| Field   | Type       | Default  | Description              |
|------------|-------------------|------------|----------------------------------------|
| `executor` | string      | `"shell"` | Executor to run the script with    |
| `dockyard` | string      | `"strict"` | Dockyard isolation level        |
| `env`   | map of strings  | `{}`    | Extra environment variables      |
| `script`  | string      | `""`    | The script to execute         |

The `env` field can use variable substitution in values:

```toml
[lifecycle.configure]
env = { CFLAGS = "-O2 -pipe", PREFIX = "/usr" }
script = """
./configure --prefix=${PREFIX}
"""
```

### `[lifecycle_order]`

Override the default pipeline order:

```toml
[lifecycle_order]
stages = ["fetch", "verify", "extract", "configure", "compile", "staging", "fabricate"]
```

### `[mvp]` — MVP Phase Overrides

The `[mvp]` section defines alternative dependencies and lifecycle scripts for the
MVP build pass, which is used to break dependency cycles.

```toml
[mvp.dependencies]
link = ["cairo", "pango", "glib", "libxml2", "harfbuzz", "freetype", "fribidi"]

[mvp.lifecycle.configure]
script = """
meson setup build \
 --prefix=/usr \
 -Dpixbuf=disabled
"""
```

MVP dependencies override the top-level `[dependencies]`. Any field omitted in
`[mvp.dependencies]` falls back to the corresponding top-level list.

Resolution order during the MVP pass:

1. If `[mvp.lifecycle.<stage>]` exists, it is used.
2. Otherwise, it falls back to `[lifecycle.<stage>]`.

#### Recommended: `mvp.toml`

For small plans, inline `[mvp]` is fine. When the MVP path becomes large or
substantially different, prefer a sibling `mvp.toml` file:

```text
foo/
├── plan.toml
└── mvp.toml
```

`mvp.toml` is a **restricted overlay**. It accepts the same fields as the body
of `[mvp]`, but without the wrapper:

```toml
[dependencies]
build = ["binutils", "glibc"]

[lifecycle.configure]
script = "..."
```

Allowed top-level fields in `mvp.toml`:

- `dependencies`
- `lifecycle`
- `lifecycle_order`

Do not duplicate part metadata, sources, outputs, or hooks there. Also do
not mix inline `[mvp]` in `plan.toml` with a sibling `mvp.toml`; choose one.

### `[lifecycle.fabricate]` — Final Build Stage

`[lifecycle.fabricate]` now describes only the final build-stage script that
runs after `staging` and before archive creation.

```toml
[lifecycle.fabricate]
script = """
find ${PART_DIR}/usr/share/doc -type f -name '*.la' -delete
"""
```

### `[hooks]` — Install / Upgrade / Remove Hooks

`[hooks]` contains transaction-time scripts that run on the live system, not in
the dockyarded build lifecycle.

```toml
[hooks]
pre_install = "echo 'Preparing installation...'"
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"
```

| Field        | Type  | Description        |
|----------------------|--------|----------------------------|
| `pre_install`    | string | Run before first install  |
| `post_install`    | string | Run after first install  |
| `post_upgrade`    | string | Run after upgrade     |
| `pre_remove`     | string | Run before part removal |
| `post_remove`    | string | Run after part removal |

Hooks run on the live system, serially, blocking the install. Keep them fast. For operations that are inherently slow and single-threaded (e.g. `fmtutil-sys --all`, `texhash`, font cache generation), prefer running only the subset needed at install time and let the user invoke the full regeneration manually afterward:

```toml
[hooks]
post_install = """
mktexlsr 2>/dev/null || true
texlinks 2>/dev/null || true
"""
# fmtutil-sys --all is intentionally omitted — run manually:
#  sudo fmtutil-sys --byfmt pdflatex
```

### `[output]` — Output Metadata & Part Relations

`[output]` defines install-time metadata for the main part, and
`[output.<name>]` declares additional split outputs.

Each output carries its own **part relations** — install-time metadata
describing how a part interacts with other parts in the system.

```toml
[output]
conflicts = ["apache"]
provides = ["http-server"]
replaces = ["old-nginx"]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]

[output."nginx-doc"]
description = "Nginx documentation"
provides = ["nginx-documentation"]
script = "..."
```

| Field     | Type      | Description               |
|----------------|-----------------|------------------------------------------|
| `replaces`   | list of strings | Parts this output replaces (auto-removed on install) |
| `conflicts`  | list of strings | Parts that cannot coexist with this output |
| `provides`   | list of strings | Virtual part names this output satisfies |
| `backup`    | list of strings | Config files preserved across upgrades  |
| `description` | string     | Sub-part description (multi-output mode) |
| `script`    | string     | Script to select/install files into the sub-part (multi-output mode) |
| `hooks.*`   | table/fields  | Transaction hooks for a sub-part   |
| `dependencies` | table      | Sub-part dependencies (multi-output mode) |

#### Part Relations

Relations are **per-output**, not per-plan. In multi-output mode, each
sub-part declares its own relations independently.

- **`replaces`** — Automatic migration. When installing this part, Wright
 silently removes any installed part whose name appears in this list.
 Use for part renames and merges (e.g. `nginx-mainline` replaces `nginx`).

- **`conflicts`** — Mutual exclusion. Wright refuses to install this part
 while a conflicting part is present (or vice versa). Use when two parts
 provide overlapping functionality and cannot coexist (e.g. `nginx` and
 `apache` both binding port 80). Conflicts are **bidirectional** — if A
 conflicts with B, installing B when A is present is also refused.

- **`provides`** — Virtual names. Allows this part to satisfy dependencies
 on an abstract capability rather than a concrete part name. Multiple parts
 can provide the same virtual name (e.g. both `nginx` and `apache` provide
 `http-server`), enabling consumers to depend on the capability without
 coupling to a specific implementation.

##### `replaces` vs `conflicts`

| | `replaces` | `conflicts` |
|---|---|---|
| **Intent** | Part rename / merge | Mutual exclusion |
| **On install** | Old part auto-removed | Install refused |
| **Direction** | One-way (new replaces old) | Bidirectional |

#### Backup files

Files listed in `backup` are treated as **user-owned config files**:

- **On upgrade:** the new default is always written alongside as `<path>.wnew`
 (e.g. `/etc/nginx/nginx.conf.wnew`) and a warning is printed. The live file is
 left intact so user customisations are never lost. The user can then diff the
 two files and merge changes manually. Files **not** listed in `backup` are
 overwritten directly.
- **On remove:** config files are **not deleted**, even when the part is removed.

## Default Lifecycle Pipeline

The default pipeline runs these stages in order:

| Stage     | Type   | Description               |
|----------------|----------|------------------------------------------|
| `fetch`    | built-in | Download sources and copy local files  |
| `verify`    | built-in | Verify SHA-256 checksums         |
| `extract`   | built-in | Extract archives, copy non-archives to `${FILES_DIR}` |
| `prepare`   | user   | Pre-build setup (e.g. apply patches)   |
| `configure`  | user   | Run configure scripts          |
| `compile`   | user   | Compile the software           |
| `check`    | user   | Run test suites             |
| `staging`   | user   | Install files into `${PART_DIR}`     |
| `fabricate`  | user   | Finalize the staged part before archiving |

Built-in stages (`fetch`, `verify`, `extract`) are handled by the build tool automatically. User stages are only run if defined in `plan.toml` — undefined stages are silently skipped. Most plans install files during `staging`; use `[lifecycle.fabricate]` only when you need a final post-staging script before archive creation.

Override this order with `[lifecycle_order]` if your build needs a different pipeline.

## Pre/Post Hooks

Any stage can have a pre- or post-hook. Name them `pre_<stage>` or `post_<stage>`:

```toml
[lifecycle.pre_compile]
script = """
echo "About to compile..."
"""

[lifecycle.compile]
script = """
make -j$(nproc)
"""

[lifecycle.post_compile]
script = """
echo "Compilation complete."
"""
```

Execution order for each stage: `pre_<stage>` → `<stage>` → `post_<stage>`. Hooks are only run if defined. They support the same fields as any lifecycle stage (`executor`, `dockyard`, `env`, `script`).

## Phase-Based Cycles (MVP → Full)

Some parts have genuine circular build-time dependencies. The classic example is `freetype` ↔ `harfbuzz`: freetype needs harfbuzz for OpenType shaping, and harfbuzz needs freetype for glyph rendering. These cycles cannot be broken by fixing dependency types — they are real.

Wright resolves them automatically using a **two-pass build**:

1. **MVP pass** — builds the part without the cyclic dependency (functional but reduced).
2. **Full pass** — after the rest of the cycle is built, rebuilds the part with all dependencies.

### Declaring an MVP phase

Define MVP-specific dependencies so the graph becomes acyclic:

```toml
[mvp.dependencies]
link = ["freetype"] # omit harfbuzz in MVP
```

Wright's orchestrator uses Tarjan's SCC algorithm to detect cycles. If it finds a cycle and a plan in that cycle has `[mvp.dependencies]` that remove at least one edge of the cycle, it automatically inserts the two-pass schedule. If no plan provides an acyclic MVP dependency set, the build fails with a clear error identifying the cycle.

The MVP phase can also be triggered **manually** without a cycle being present, using the `--mvp` flag:

```bash
wright build freetype --mvp
```

This builds using `[mvp.dependencies]` and sets the same `WRIGHT_BUILD_PHASE=mvp` environment variables as an automatic cycle-breaking pass. It is useful for testing that a plan's MVP configuration is correct before it is needed in a real cycle.

### Phase environment variables

During the MVP pass, Wright injects these variables into every lifecycle stage:

| Variable | Value | Description |
|----------|-------|-------------|
| `WRIGHT_BUILD_PHASE` | `mvp` | Phase name for the MVP pass (`full` in the normal pass) |
| `WRIGHT_BOOTSTRAP_WITHOUT_<DEP>` | `1` | Set for each excluded dependency (name uppercased, hyphens → underscores) |

The plan script can still use these variables to disable the relevant feature:

```toml
[mvp.dependencies]
link = ["freetype"]

[lifecycle.configure]
script = """
cmake -B build \
  ${WRIGHT_BOOTSTRAP_WITHOUT_HARFBUZZ:+-DFREETYPE_WITH_HARFBUZZ=OFF} \
  -DCMAKE_INSTALL_PREFIX=/usr
"""
```

### MVP lifecycle overrides (recommended)

For complex parts, it is safer to provide **dedicated MVP scripts** instead of
embedding conditionals. Wright supports a `[mvp.lifecycle]` section that overrides
`[lifecycle]` **only during the MVP pass**.

```toml
[mvp.dependencies]
link = ["cairo", "pango", "glib", "libxml2", "harfbuzz", "freetype", "fribidi"]

[lifecycle.configure]
script = """
meson setup build \
 --prefix=/usr \
 -Dpixbuf=enabled
"""

[mvp.lifecycle.configure]
script = """
meson setup build \
 --prefix=/usr \
 -Dpixbuf=disabled \
 -Dpixbuf-loader=disabled
"""
```

Resolution order for the MVP pass:

1. If `[mvp.lifecycle.<stage>]` exists, it is used.
2. Otherwise, it falls back to `[lifecycle.<stage>]`.

This keeps the **MVP build** separate from the **full build**, and avoids
fragile shell conditionals.

### Dependency Graph Analysis

`wright build --lint` validates each plan and prints a dependency graph report:

- Whether the graph is acyclic
- Each detected cycle (if any)
- MVP candidates that would break the cycle
- The selected candidate (deterministic: fewest excluded edges, then name)

### Construction Plan output

When a cycle is resolved, the scheduling log shows the two-pass schedule by
dependency wave:

```
INFO Build batch 1/2: bootstrap freetype, build harfbuzz.
INFO Build batch 2/2: full rebuild freetype.
```

The `bootstrap` action is the first pass (incomplete). `full rebuild` is the
second pass (complete, automatically force-rebuilt). MVP builds are never
written to the build cache.

When `--mvp` is used explicitly, all targets show `bootstrap` in the batch
summary and no `full rebuild` pass follows:

```
INFO Build batch 1/1: bootstrap freetype.
```

### Dependency type classification comes first

Most apparent cycles are caused by incorrect dependency classification. Before defining phase-specific dependencies, verify that:

- **`link`** is only used for shared libraries your binary actually links against at build time.
- **`runtime`** is used for plugins, loaders, and tools called at runtime.

For example, `gdk-pixbuf` using glycin (an image loader plugin) as a `link` dependency creates a false cycle. The correct fix is `runtime = ["glycin"]`, not a phase override.

Reserve phase-specific dependencies for cycles that remain after dependency types are correct.

## Variable Substitution

Variables use `${VAR_NAME}` syntax and are expanded in scripts and source URIs. Unrecognized variables are left as-is.

| Variable    | Description                |
|-----------------|--------------------------------------------|
| `${NAME}`  | Current output name from `name` / `[output.<name>]` |
| `${VERSION}`| Version from `version`     |
| `${RELEASE}`| Release number as a string         |
| `${ARCH}`  | Target architecture            |
| `${SRC_DIR}`  | Extraction root directory         |
| `${BUILD_DIR}` | Top-level source directory (use this in scripts) |
| `${PART_DIR}`  | Current output staging directory |
| `${MAIN_PART_NAME}` | Primary output name from the top-level `name` field |
| `${MAIN_PART_DIR}` | Primary output staging directory (`${PART_DIR}` outside split outputs) |
| `${FILES_DIR}` | Directory containing non-archive files (patches, configs, etc.) |
| `${WRIGHT_BUILD_PHASE}` | Current phase name (`full` or `mvp`) |
| `${WRIGHT_BOOTSTRAP_WITHOUT_<DEP>}` | Set to `1` for each dep excluded in the MVP pass |

When running inside a dockyard, path variables are remapped to dockyard mount points:

| Variable    | Host value       | Dockyard value     |
|-----------------|------------------------|------------------------|
| `${SRC_DIR}`  | actual host path    | `/build`        |
| `${BUILD_DIR}` | actual host path    | `/build/<source-dir>` |
| `${PART_DIR}`  | actual host path    | `/output`       |
| `${MAIN_PART_DIR}` | actual host path | `/output` or `/main-pkg` in split-output scripts |
| `${FILES_DIR}` | actual host path    | `/files`        |

`${BUILD_DIR}` points to the top-level directory extracted from the source archive. For example, if `nginx-1.25.3.tar.gz` extracts to `nginx-1.25.3/`, then `${BUILD_DIR}` is `${SRC_DIR}/nginx-1.25.3`. If the archive extracts files directly without a top-level directory, `${BUILD_DIR}` equals `${SRC_DIR}`. Use `${BUILD_DIR}` instead of manually `cd`-ing into the source directory.

Additionally, the following host environment variables are passed through to the build if set: `CC`, `CXX`, `AR`, `AS`, `LD`, `NM`, `RANLIB`, `STRIP`, `OBJCOPY`, `OBJDUMP`, `CFLAGS`, `CXXFLAGS`, `CPPFLAGS`, `LDFLAGS`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `LIBRARY_PATH`, `PKG_CONFIG_PATH`, `PKG_CONFIG_SYSROOT_DIR`, `MAKEFLAGS`, `JOBS`.

## Dockyard Levels

The `dockyard` field on each lifecycle stage controls process isolation:

### `none`

No isolation. The script runs directly on the host. Use this only when dockyard support is unavailable or for stages that need full host access.

### `relaxed`

Partial isolation using Linux namespaces:
- Mount namespace (private mounts)
- PID namespace (isolated process tree)
- UTS namespace (isolated hostname)

Network and IPC remain shared with the host.

### `strict` (default)

Full isolation. Includes everything in `relaxed` plus:
- Network namespace (no network access)
- IPC namespace (isolated System V IPC and POSIX message queues)

In both `relaxed` and `strict` modes, the dockyard:
- Pivots to a minimal root filesystem
- Bind-mounts `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64` read-only
- Bind-mounts essential `/etc` files (`resolv.conf`, `hosts`, `passwd`, `group`, `ld.so.conf`, `ld.so.cache`) read-only
- Mounts the source directory at `/build` (read-write)
- Mounts the part output directory at `/output` (read-write)
- Mounts the files directory at `/files` (read-only, if present)
- Provides `/dev` with basic devices (`null`, `zero`, `urandom`, `random`, `full`)
- Mounts a fresh `/proc` and `/tmp`
- Sets hostname to `wright-dockyard`

If the kernel does not support the required namespaces (e.g. inside a container), the dockyard falls back to direct execution with a warning.

### Choosing a level

| Build tool / scenario | Level | Reason |
|-----------------------|-------|--------|
| C/C++ — autotools, CMake, meson | `strict` | No network needed at build time; default is correct |
| Rust — `cargo build` | `relaxed` | Cargo fetches crates from crates.io during compilation unless vendored |
| Go — `go build` / `go mod download` | `relaxed` | Go modules download from proxy.golang.org during build unless vendored |
| Node.js — `npm install` / `yarn` | `relaxed` | Package manager downloads from npm registry during install |
| Python — `pip install` / `python setup.py` | `relaxed` | pip fetches from PyPI during install |
| Stage needs host IPC | `relaxed` | IPC namespace is not isolated, so System V / POSIX queues remain accessible |
| Stage needs full host access | `none` | No namespace isolation at all — use only when unavoidable |

The recommended pattern for network-fetching build tools (Cargo, Go, npm) is to
pre-vendor dependencies and build fully offline under `strict`:

- **Cargo**: include a `vendor/` directory and set `CARGO_NET_OFFLINE=true` plus a
 `.cargo/config.toml` pointing at the vendor dir.
- **Go**: run `go mod vendor` and pass `-mod=vendor` at build time.
- **npm**: include `node_modules/` in the source archive or use `npm pack`/offline mirror.

When vendoring is not practical (e.g. bootstrapping the toolchain itself), use
`relaxed` so the build can reach the network while still keeping a private
filesystem and process namespace.

## Executors

Executors define how scripts are run. The `executor` field on a lifecycle stage selects which executor to use.

### Built-in: `shell`

The default executor. Runs scripts with `/bin/bash -e -o pipefail`, so any failing command aborts the stage. Scripts are written to a temporary `.sh` file and passed as an argument to bash.

### Custom Executors

Additional executors (e.g. `python`, `lua`) can be installed as TOML files in the executor directory. Each file defines:

```toml
[executor]
name = "python"
description = "Python script executor"
command = "/usr/bin/python3"
args = []
delivery = "tempfile"
tempfile_extension = ".py"
required_paths = ["/usr/lib/python3"]
default_dockyard = "strict"
```

| Field       | Type      | Default   | Description             |
|--------------------|-----------------|--------------|--------------------------------------|
| `name`       | string     | required   | Executor name used in lifecycle stages |
| `description`   | string     | `""`     | Human-readable description      |
| `command`     | string     | required   | Path to the interpreter       |
| `args`       | list of strings | `[]`     | Arguments before the script path   |
| `delivery`     | string     | `"tempfile"` | How the script is passed to the command |
| `tempfile_extension`| string     | `".sh"`   | File extension for the temp script  |
| `required_paths`  | list of strings | `[]`     | Extra paths to bind-mount in the dockyard |
| `default_dockyard` | string     | `""`     | Default dockyard isolation level for this executor |

Reference a custom executor by name:

```toml
[lifecycle.configure]
executor = "python"
script = """
import os
os.makedirs(f"{os.environ['PART_DIR']}/usr/lib", exist_ok=True)
"""
```

## Examples

### Minimal Plan

```toml
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test part"
license = "MIT"
arch = "x86_64"

[dependencies]
build = ["gcc"]

[lifecycle.prepare]
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\n"); return 0; }
EOF
"""

[lifecycle.compile]
script = """
gcc -o hello hello.c
"""

[lifecycle.staging]
script = """
install -Dm755 hello ${PART_DIR}/usr/bin/hello
"""
```

### Real-World Plan (nginx)

```toml
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Example Maintainer <maintainer@example.com>"

[dependencies]
runtime = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]
build = ["perl", "gcc", "make"]
optional = ["geoip"]

[[sources]]
uri = "https://nginx.org/download/nginx-${VERSION}.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"

[[sources]]
uri = "patches/fix-headers.patch"
sha256 = "SKIP"

[options]
static = false
debug = false
ccache = true

[lifecycle.prepare]
script = """
cd ${BUILD_DIR}
patch -Np1 < ${FILES_DIR}/fix-headers.patch
patch -Np1 < ${FILES_DIR}/add-feature.patch
"""

[lifecycle.configure]
env = { CFLAGS = "-O2 -pipe" }
script = """
cd ${BUILD_DIR}
./configure --prefix=/usr
"""

[lifecycle.compile]
script = """
cd ${BUILD_DIR}
make -j$(nproc)
"""

[lifecycle.check]
script = """
cd ${BUILD_DIR}
make test
"""

[lifecycle.staging]
script = """
cd ${BUILD_DIR}
make DESTDIR=${PART_DIR} install
"""

[hooks]
pre_install = "echo 'Preparing nginx installation...'"
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"

[output]
conflicts = ["apache"]
provides = ["http-server"]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

### Multi-Package Mode

A single plan can produce multiple output parts. This avoids rebuilding the same source just to partition files into separate archives. Common use cases: separating documentation, libraries, or development headers from the main part.

In multi-output mode, the main part uses `[output]`, and extra outputs are
declared as subtables of `[output]`.

```toml
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[lifecycle.compile]
script = "make -j$(nproc)"

[lifecycle.staging]
script = """
cd ${BUILD_DIR}
make DESTDIR=${PART_DIR} install
"""

[hooks]
post_install = "..."

[output."libstdc++"]
description = "GNU C++ standard library"
script = "install -Dm755 libstdc++.so ${PART_DIR}/usr/lib/libstdc++.so"
hooks.post_install = "ldconfig"
dependencies.runtime = ["libgcc"]
```

Sub-parts inherit `version`, `release`, `arch`, and `license` from the
parent manifest unless overridden. Each sub-part can have a `description`, a
`script` to select/install files, `hooks.*` fields, `backup`, and a
`dependencies` table. Names containing `+` or `.` must be quoted in TOML table
headers (e.g. `[output."libstdc++"]`).

Sub-part dependencies use dotted keys (`dependencies.runtime`) or a sub-table
(`[output.<name>.dependencies]`) for parts that must be installed when this
sub-part is installed independently.

```toml
[lifecycle.staging]
script = "cd ${BUILD_DIR} && make DESTDIR=${PART_DIR} install"

[output."libfoo-dev"]
description = "Development headers for libfoo"
script = """
install -Dm644 ${BUILD_DIR}/include/* ${PART_DIR}/usr/include/libfoo/
install -Dm644 ${BUILD_DIR}/libfoo.pc ${PART_DIR}/usr/lib/pkgconfig/libfoo.pc
"""
```

Sub-parts are independent archives — installing the parent does **not** automatically install its sub-parts. To create a meta-part that pulls in all sub-parts, list them as `runtime` dependencies on the parent:

```toml
name = "linux-firmware"
# ...

[dependencies]
runtime = ["linux-firmware-amd", "linux-firmware-intel", "linux-firmware-nvidia"]

[lifecycle.fabricate.linux-firmware-amd]
description = "AMD GPU/CPU firmware"
# ...
```

In this pattern the parent part itself may contain no files — it exists only to group the sub-parts.

For a `-doc` sub-part that overrides the architecture:

```toml
[lifecycle.fabricate.mypackage-doc]
description = "Documentation for mypackage"
arch = "any"
script = """
install -Dm644 ${BUILD_DIR}/docs/* ${PART_DIR}/usr/share/doc/mypackage/
"""
```

## Validation Rules

Wright validates `plan.toml` on parse. A plan that fails validation cannot be built.

| Rule | Detail |
|------|--------|
| **name** | Must match `[a-z0-9][a-z0-9_+.-]*`, max 64 characters. Names containing `+` or `.` must be quoted in TOML table headers (e.g. `[lifecycle.fabricate."libstdc++"]`). |
| **version** | Any non-empty string containing alphanumeric characters (e.g. `1.25.3`, `6.5-20250809`, `2024a`) |
| **release** | Must be >= 1 |
| **epoch** | Must be >= 0 (default 0) |
| **description** | Must not be empty |
| **license** | Must not be empty |
| **arch** | Must not be empty |
| **sha256** | Each `[[sources]]` entry has its own `sha256` (use `"SKIP"` for local paths and git sources) |

The output archive is named `{name}-{version}-{release}-{arch}.wright.tar.zst`. When `epoch` > 0, the filename includes it: `{name}-{epoch}:{version}-{release}-{arch}.wright.tar.zst`.
