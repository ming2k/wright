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
| `version`   | string  | yes   | —    | Dependency version (free-form)    |
| `release`   | integer | yes   | —    | Build revision (must be >= 1)   |
| `epoch`    | integer | no    | `0`   | Version epoch — overrides version comparison (see below) |
| `description` | string  | yes   | —    | Short description (must not be empty) |
| `license`   | string  | yes   | —    | SPDX license identifier      |
| `arch`    | string  | yes   | —    | Target architecture (e.g. `x86_64`) |
| `url`     | string  | no    | —    | Dependency project URL        |
| `maintainer` | string  | no    | —    | Maintainer name and email     |

#### Epoch

The `epoch` field forces a part to be considered newer than any version with a lower epoch, regardless of the version string. This is needed when dependency changes their versioning scheme in a way that makes the new version sort lower (e.g. a rename from `2024.1` to `1.0.0`).

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

Sources use TOML's array-of-tables syntax with a mandatory `type` field to define the protocol. Wright supports three source types: `http`, `git`, and `local`.

All URLs and paths support variable substitution (see [Variable Substitution](#variable-substitution)).

#### `type = "http"`

Used for downloading remote tarballs or single files.

| Field   | Type   | Default  | Description                                                                 |
|-----------|--------|----------|-----------------------------------------------------------------------------|
| `url`     | string | required | Remote URL (`http://` or `https://`)                                      |
| `sha256`  | string | required | SHA-256 checksum. Use `"SKIP"` only during development or for untrusted sources. |
| `as`      | string | optional | Rename the downloaded file in the cache.                                    |
| `extract_to` | string | optional | Subdirectory under `${WORKDIR}` to extract/copy the file into.          |

```toml
[[sources]]
type = "http"
url = "https://nginx.org/download/nginx-${VERSION}.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"
```

#### `type = "git"`

Used for cloning Git repositories. Git sources are automatically extracted (checked out) to the build directory.

| Field   | Type   | Default  | Description                                                                 |
|-----------|--------|----------|-----------------------------------------------------------------------------|
| `url`     | string | required | Git repository URL                                                         |
| `ref`     | string | `"HEAD"` | Branch, tag, or commit hash to check out.                                   |
| `depth`   | integer| optional | Shallow clone depth.                                                        |
| `extract_to` | string | optional | Subdirectory under `${WORKDIR}` to check out the repository into.         |

```toml
[[sources]]
type = "git"
url = "https://github.com/example/repo.git"
ref = "v1.2.3"
depth = 1
```

#### `type = "local"`

Used for including files located within the plan directory (e.g., patches, custom configs).

| Field   | Type   | Default  | Description                                                                 |
|-----------|--------|----------|-----------------------------------------------------------------------------|
| `path`    | string | required | Path relative to the plan directory. Must not escape the plan directory.    |
| `extract_to` | string | optional | Subdirectory under `${WORKDIR}` to copy the file into.                  |

```toml
[[sources]]
type = "local"
path = "patches/fix-headers.patch"
```

#### Archive Handling

- **Archives**: Files with supported extensions (`.tar.gz`, `.tgz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.tar.lz`, `.zip`) are automatically extracted during the `extract` stage.
- **Single Files**: Non-archive files are copied directly to `${WORKDIR}` (or the subdirectory specified by `extract_to`), preserving their natural filename.

#### Applying patches

Patches are **not** auto-applied. Include them as `type = "local"` entries and apply them manually in a lifecycle stage. This gives full control over strip level, ordering, and conditions:

```toml
[[sources]]
type = "http"
url = "https://example.com/foo-${VERSION}.tar.gz"
sha256 = "abc123..."

[[sources]]
type = "local"
path = "patches/fix-headers.patch"

[[sources]]
type = "local"
path = "patches/add-feature.patch"

[lifecycle.prepare]
script = """
patch -Np1 < ${WORKDIR}/fix-headers.patch
patch -Np1 < ${WORKDIR}/add-feature.patch
"""
```

For patches that need a different strip level:

```toml
[lifecycle.prepare]
script = """
patch -Np0 < ${WORKDIR}/special-fix.patch
patch -Np1 < ${WORKDIR}/normal-fix.patch
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
| `skip_fhs_check`  | bool      | `false` | Skip FHS validation after the implicit output slicing phase. Use only for parts with a deliberate reason to install outside standard paths (e.g. kernel modules). |

Per-plan values override global (`wright.toml`) settings. `memory_limit` and `cpu_time_limit` are enforced via `setrlimit()` before `exec` and inherited by child processes. The wall-clock `timeout` is enforced by the parent process — it catches builds stuck on I/O or deadlocks where CPU time does not advance.

**CPU parallelism:** Wright pins each isolation process to its computed CPU share via `sched_setaffinity`, so `nproc` inside the isolation already returns the correct count. Scripts should call `make -j$(nproc)` directly. To override parallelism for a specific part, set `MAKEFLAGS` (or the relevant tool variable) in `[options.env]`. See [resource-allocation.md](resource-allocation.md) for details.

**Practical guidance:** `timeout` is the most important safety net. `memory_limit` limits virtual address space (`RLIMIT_AS`), not physical RSS — set it generously (2-3x expected usage), as programs like rustc, JVM, and Go reserve large virtual mappings they never touch.

### `[lifecycle.<stage>]`

Each lifecycle stage is a TOML table under `lifecycle`:

```toml
[lifecycle.compile]
executor = "shell"
isolation = "strict"
script = """
make -j$(nproc)
"""
```

| Field   | Type       | Default  | Description              |
|------------|-------------------|------------|----------------------------------------|
| `executor` | string      | `"shell"` | Executor to run the script with    |
| `isolation` | string      | `"strict"` | Security isolation level        |
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
stages = ["fetch", "verify", "extract", "configure", "compile", "staging"]
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

### `[hooks]` — Install / Upgrade / Remove Hooks

`[hooks]` contains transaction-time scripts that run on the live system, not in
the isolated build lifecycle.

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
include = ["/usr/share/doc/.*", "/usr/share/man/.*"]
```

| Field     | Type      | Description               |
|----------------|-----------------|------------------------------------------|
| `replaces`   | list of strings | Parts this output replaces (auto-removed on install) |
| `conflicts`  | list of strings | Parts that cannot coexist with this output |
| `provides`   | list of strings | Virtual part names this output satisfies |
| `backup`    | list of strings | Config files preserved across upgrades  |
| `description` | string     | Sub-part description (multi-output mode) |
| `include`   | list of strings | Regex patterns for files to include in this sub-part (multi-output mode) |
| `exclude`   | list of strings | Regex patterns for files to exclude from this sub-part (multi-output mode) |
| `hooks.*`   | table/fields  | Transaction hooks for a sub-part   |
| `dependencies` | table      | Additional sub-part dependencies (sub-parts automatically inherit all dependencies from the parent) |

#### Implicit Slicing (Declarative Outputs)

Wright uses a **Single-Source Staging, Multi-Target Slicing** architecture. 
Instead of writing explicit scripts to move files between packages, all files should be installed to the default `${PART_DIR}` during the `staging` lifecycle phase.

After `staging` is complete, an implicit slicing engine processes the files based on the `[output.<name>]` definitions. The engine evaluates files in `${PART_DIR}` against the `include` and `exclude` regular expressions. Files that match are automatically moved out of `${PART_DIR}` into the respective sub-part directories. The main `[output]` implicitly contains whatever remains after all sub-parts have been sliced out.

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
| `extract`   | built-in | Extract archives, copy non-archives to `${WORKDIR}` |
| `prepare`   | user   | Pre-build setup (e.g. apply patches)   |
| `configure`  | user   | Run configure scripts          |
| `compile`   | user   | Compile the software           |
| `check`    | user   | Run test suites             |
| `staging`   | user   | Install files into `${PART_DIR}`     |

Built-in stages (`fetch`, `verify`, `extract`) are handled by the build tool automatically. User stages are only run if defined in `plan.toml` — undefined stages are silently skipped. All file system layout operations (such as `make install`, path moving, and symlink creation) should be performed within the `staging` phase. After staging, an implicit and declarative "output slicing" engine processes the resulting files according to the `[output]` blocks to construct the final archives.

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

Execution order for each stage: `pre_<stage>` → `<stage>` → `post_<stage>`. Hooks are only run if defined. They support the same fields as any lifecycle stage (`executor`, `isolation`, `env`, `script`).

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
second pass (complete, automatically force-rebuilt). MVP builds produce an incomplete part that is force-rebuilt in the second pass.

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
| `${WORKDIR}`  | Extraction root directory         |
| `${PART_DIR}`  | Current output staging directory |
| `${MAIN_PART_NAME}` | Primary output name from the top-level `name` field |
| `${MAIN_PART_DIR}` | Primary output staging directory (`${PART_DIR}` outside split outputs) |
| `${WRIGHT_BUILD_PHASE}` | Current phase name (`full` or `mvp`) |
| `${WRIGHT_BOOTSTRAP_WITHOUT_<DEP>}` | Set to `1` for each dep excluded in the MVP pass |

## Path Variables

Wright uses standard variables to refer to build directories. When running inside an isolation, these paths are remapped to dedicated mount points:

| Variable    | Host value (Default) | Isolation value | Description |
|-------------|----------------------|-----------------|-------------|
| `${WORKDIR}` | `/var/tmp/wright/workshop/<name>-<version>/work` | `/build` | The root container for all sources. |
| `${PART_DIR}` | `/var/tmp/wright/workshop/<name>-<version>/output` | `/output` | The installation target directory (DESTDIR). |

### Path Mapping Note
Inside the **isolation environment**, the filesystem is restricted:
- **`/build`** is a read-write mount of the host's build work directory.
- **`/output`** is a read-write mount where build products should be installed.

### Navigating the Build
Wright **never** automatically enters subdirectories within `${WORKDIR}`. Scripts are always executed at the root of `${WORKDIR}` (mapped to `/build`). 

If your source archive extracts into a subdirectory, you must explicitly change into it:

```toml
[[sources]]
type = "http"
url = "https://example.com/nginx-1.25.3.tar.gz"
sha256 = "..."

[lifecycle.configure]
script = """
cd nginx-1.25.3
./configure --prefix=/usr
"""
```

For absolute deterministic behavior across versions, use `extract_to`:

```toml
[[sources]]
type = "http"
url = "https://.../nginx-1.25.3.tar.gz"
sha256 = "..."
extract_to = "src"

[lifecycle.configure]
script = """
cd src
./configure --prefix=/usr
"""
```


Additionally, the following host environment variables are passed through to the build if set: `CC`, `CXX`, `AR`, `AS`, `LD`, `NM`, `RANLIB`, `STRIP`, `OBJCOPY`, `OBJDUMP`, `CFLAGS`, `CXXFLAGS`, `CPPFLAGS`, `LDFLAGS`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `LIBRARY_PATH`, `PKG_CONFIG_PATH`, `PKG_CONFIG_SYSROOT_DIR`, `MAKEFLAGS`, `JOBS`.

## Isolation Levels

The `isolation` field on each lifecycle stage controls process isolation:

### `none`

No isolation. The script runs directly on the host. Use this only when isolation support is unavailable or for stages that need full host access.

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

In both `relaxed` and `strict` modes, the isolation:
- Pivots to a minimal root filesystem
- Bind-mounts `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64` read-only
- Bind-mounts essential `/etc` files (`resolv.conf`, `hosts`, `passwd`, `group`, `ld.so.conf`, `ld.so.cache`) read-only
- Mounts the source directory at `/build` (read-write)
- Mounts the part output directory at `/output` (read-write)
- Mounts the work directory at `/files` (read-only, if present)
- Provides `/dev` with basic devices (`null`, `zero`, `urandom`, `random`, `full`)
- Mounts a fresh `/proc` and `/tmp`
- Sets hostname to `wright-isolation`

If the kernel does not support the required namespaces (e.g. inside a container), the isolation falls back to direct execution with a warning.

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
default_isolation = "strict"
```

| Field       | Type      | Default   | Description             |
|--------------------|-----------------|--------------|--------------------------------------|
| `name`       | string     | required   | Executor name used in lifecycle stages |
| `description`   | string     | `""`     | Human-readable description      |
| `command`     | string     | required   | Path to the interpreter       |
| `args`       | list of strings | `[]`     | Arguments before the script path   |
| `delivery`     | string     | `"tempfile"` | How the script is passed to the command |
| `tempfile_extension`| string     | `".sh"`   | File extension for the temp script  |
| `required_paths`  | list of strings | `[]`     | Extra paths to bind-mount in the isolation |
| `default_isolation` | string     | `""`     | Default isolation isolation level for this executor |

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
type = "http"
url = "https://nginx.org/download/nginx-${VERSION}.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"

[[sources]]
type = "local"
path = "patches/fix-headers.patch"

[options]
static = false
debug = false
ccache = true

[lifecycle.prepare]
script = """
patch -Np1 < ${WORKDIR}/fix-headers.patch
patch -Np1 < ${WORKDIR}/add-feature.patch
"""

[lifecycle.configure]
env = { CFLAGS = "-O2 -pipe" }
script = """
./configure --prefix=/usr
"""

[lifecycle.compile]
script = """
make -j$(nproc)
"""

[lifecycle.check]
script = """
make test
"""

[lifecycle.staging]
script = """
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

### Multi-Output Mode

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

During sub-part staging, the main part's output is mounted read-only at
`/main-part` (and available via `${MAIN_PART_DIR}`).

Sub-part dependencies use dotted keys (`dependencies.runtime`) or a sub-table
(`[output.<name>.dependencies]`) for parts that must be installed when this
sub-part is installed independently.

```toml
[lifecycle.staging]

[output."libfoo-dev"]
description = "Development headers for libfoo"
script = """
mv ${MAIN_PART_DIR}/usr/include ${PART_DIR}/usr/
"""
```

Sub-parts are independent archives — installing the parent does **not** automatically install its sub-parts. To create a meta-part that pulls in all sub-parts, list them as `runtime` dependencies on the parent:

```toml
name = "linux-firmware"
# ...

[dependencies]
runtime = ["linux-firmware-amd", "linux-firmware-intel", "linux-firmware-nvidia"]

[output."linux-firmware-amd"]
description = "AMD GPU/CPU firmware"
# ...
```

In this pattern the parent part itself may contain no files — it exists only to group the sub-parts.

For a `-doc` sub-part that overrides the architecture:

```toml
[output."mypart-doc"]
description = "Documentation for mypart"
arch = "any"
script = """
mv ${MAIN_PART_DIR}/usr/share/doc ${PART_DIR}/usr/share/
"""
```

## Validation Rules

Wright validates `plan.toml` on parse. A plan that fails validation cannot be built.

| Rule | Detail |
|------|--------|
| **name** | Must match `[a-z0-9][a-z0-9_+.-]*`, max 64 characters. Names containing `+` or `.` must be quoted in TOML table headers (e.g. `[output."libstdc++"]`). |
| **version** | Any non-empty string containing alphanumeric characters (e.g. `1.25.3`, `6.5-20250809`, `2024a`) |
| **release** | Must be >= 1 |
| **epoch** | Must be >= 0 (default 0) |
| **description** | Must not be empty |
| **license** | Must not be empty |
| **arch** | Must not be empty |
| **sha256** | Each `[[sources]]` entry has its own `sha256` (use `"SKIP"` for local paths and git sources) |

The output archive is named `{name}-{version}-{release}-{arch}.wright.tar.zst`. When `epoch` > 0, the filename includes it: `{name}-{epoch}:{version}-{release}-{arch}.wright.tar.zst`.

## Best Practices & Conventions

To keep your plans clean and robust across version updates, follow these organizational patterns.

### Source Organization Style

#### 1. Single Main Archive (Recommended)
For plans with one primary source code archive, always extract it to a directory named `source`.
- **Convention**: Use `extract_to = "source"`.
- **Benefit**: You can hardcode `cd source` in your scripts. It works regardless of whether the dependency folder is named `app-1.0` or `app-v2.0-final`.

```toml
[[sources]]
type = "http"
url = "https://example.com/myapp-v${VERSION}.tar.gz"
sha256 = "..."
extract_to = "source"

[lifecycle.compile]
script = "cd source && make"
```

#### 2. Patches and Single Files
Do **not** use `extract_to` for individual files like patches or configuration templates.
- **Convention**: Leave `extract_to` unset.
- **Benefit**: Files are placed directly in `${WORKDIR}`, making them easy to reference (e.g., `${WORKDIR}/fix.patch`).

```toml
[[sources]]
type = "local"
path = "fix-build.patch"

[lifecycle.compile]
script = "cd source && patch -p1 < ${WORKDIR}/fix-build.patch"
```

#### 3. Multi-Component Builds
For complex builds involving multiple archives, give each one a unique, descriptive directory name.
- **Convention**: Use specific names like `app`, `modules`, or `data`.

```toml
[[sources]]
type = "http"
url = "https://example.com/core.tar.gz"
sha256 = "..."
extract_to = "core"

[[sources]]
type = "http"
url = "https://example.com/extra-plugin.tar.gz"
sha256 = "..."
extract_to = "plugins/extra"
```

### Scripting Robustness
- **Explicit Navigation**: Now that automatic directory detection is removed, always start your scripts with an explicit `cd` if your work is in a subdirectory.
- **Variable Usage**: Prefer `${WORKDIR}/filename` over relative paths for clarity.
- **Cleanup**: Don't worry about cleaning up `${WORKDIR}` or `${PART_DIR}`; Wright handles this automatically before each build.

