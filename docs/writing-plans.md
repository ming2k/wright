# Writing Plans

A **plan** is a directory containing a `plan.toml` file that describes how to fetch, build, and package a piece of software. This guide is the complete reference for plan authors.

## Directory Structure

Plans live in a flat directory tree. Each plan is a directory named after the package:

```
plans/
├── hello/
│   └── plan.toml
├── nginx/
│   ├── plan.toml
│   └── patches/
│       └── fix-headers.patch
└── python/
    ├── plan.toml
    └── patches/
        ├── 001-fix-paths.patch
        └── 002-no-rpath.patch
```

The directory name should match the `name` field in `plan.toml`. Local files referenced in `[sources].uris` are relative to the plan directory and must not escape it.

## `plan.toml` Reference

### `[plan]` — Metadata

| Field         | Type     | Required | Default | Description                        |
|---------------|----------|----------|---------|------------------------------------|
| `name`        | string   | yes      | —       | Package name                       |
| `version`     | string   | yes      | —       | Upstream version (free-form)       |
| `release`     | integer  | yes      | —       | Build revision (must be >= 1)      |
| `description` | string   | yes      | —       | Short description (must not be empty) |
| `license`     | string   | yes      | —       | SPDX license identifier           |
| `arch`        | string   | yes      | —       | Target architecture (e.g. `x86_64`) |
| `url`         | string   | no       | —       | Upstream project URL               |
| `maintainer`  | string   | no       | —       | Maintainer name and email          |

### `[dependencies]`

All fields default to empty lists if omitted.

| Field       | Type                            | Description                          |
|-------------|---------------------------------|--------------------------------------|
| `runtime`   | list of strings                 | Must be installed at runtime (e.g. bash, python) |
| `build`     | list of strings                 | Required only during build (e.g. gcc, cmake) |
| `link`      | list of strings                 | Shared library dependencies. Triggers rebuild on update. |
| `replaces`  | list of strings                 | Packages that this one replaces (automatically uninstalled) |
| `optional`  | list of `{name, description}`   | Optional runtime dependencies        |
| `conflicts` | list of strings                 | Packages that cannot be installed alongside this one |
| `provides`  | list of strings                 | Virtual packages this one provides   |

#### `link` dependencies vs `runtime`

- **`link`**: Use this for shared libraries (`.so`) that your program links against. Wright will **automatically rebuild** your package whenever a `link` dependency is updated, ensuring ABI compatibility. It also provides CRITICAL protection against removal.
- **`runtime`**: Use this for tools or scripts called at runtime (e.g. a Python script needing `python`). Updating a `runtime` dependency does not trigger a rebuild.

#### `replaces` vs `conflicts`

- **`replaces`**: Use this for package renames or merges. If a package in this list is already installed, Wright will **automatically uninstall** it before installing the current package.
- **`conflicts`**: Use this when two packages provide similar functionality but cannot coexist (e.g. `nginx` and `apache` both wanting port 80). Wright will **refuse to install** the package if a conflicting one is already present.

#### Version constraints

Runtime, build, link, conflicts, and provides entries can include a version constraint:

```toml
link = ["openssl >= 3.0"]
runtime = ["python >= 3.10"]
```

Supported operators: `>=`, `<=`, `>`, `<`, `=`.

#### Optional dependencies

Optional dependencies use an inline table with `name` and `description`:

```toml
optional = [
    { name = "geoip", description = "GeoIP module support" },
]
```

### `[sources]`

| Field     | Type            | Default | Description                              |
|-----------|-----------------|---------|------------------------------------------|
| `uris`    | list of strings | `[]`    | Source URIs — remote URLs (`http://`/`https://`) or local paths relative to the plan directory |
| `sha256`  | list of strings | `[]`    | SHA-256 checksums (one per URI, in order). Use `"SKIP"` for local files. |

URIs support variable substitution (see [Variable Substitution](#variable-substitution)):

```toml
uris = ["https://nginx.org/download/nginx-${PKG_VERSION}.tar.gz"]
```

Use `"SKIP"` as a sha256 entry to skip verification for a specific source (required for local paths):

```toml
uris = [
    "https://example.com/foo-${PKG_VERSION}.tar.gz",
    "patches/fix-headers.patch",
]
sha256 = [
    "abc123...",
    "SKIP",
]
```

#### URI classification

- **Remote URIs** (starting with `http://` or `https://`) are downloaded to the source cache.
- **Local URIs** (everything else) are resolved relative to the plan directory. They must not escape the plan directory (path traversal is blocked).

#### Archive vs non-archive URIs

- URIs pointing to archive files (`.tar.gz`, `.tgz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`) are extracted to the source directory during the `extract` stage.
- Non-archive URIs (patches, config files, scripts, etc.) are copied to `${FILES_DIR}` where lifecycle scripts can access them.

#### Applying patches

Patches are **not** auto-applied. Include them in `uris` and apply them manually in a lifecycle stage. This gives full control over strip level, ordering, and conditions:

```toml
[sources]
uris = [
    "https://example.com/foo-${PKG_VERSION}.tar.gz",
    "patches/fix-headers.patch",
    "patches/add-feature.patch",
]
sha256 = [
    "abc123...",
    "SKIP",
    "SKIP",
]

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

| Field               | Type            | Default | Description                              |
|---------------------|-----------------|---------|------------------------------------------|
| `strip`             | bool            | `true`  | Strip debug symbols from binaries        |
| `static`            | bool            | `false` | Build statically linked binaries         |
| `debug`             | bool            | `false` | Build with debug info                    |
| `ccache`            | bool            | `true`  | Use ccache for compilation if available  |
| `env`               | map of strings  | `{}`    | Environment variables injected into every lifecycle stage |
| `memory_limit`      | integer         | —       | Max virtual address space per build process (MB), overrides global |
| `cpu_time_limit`    | integer         | —       | Max CPU time per build process (seconds), overrides global |
| `timeout`           | integer         | —       | Wall-clock timeout per build stage (seconds), overrides global |

Per-plan values override global (`wright.toml`) settings. `memory_limit` and `cpu_time_limit` are enforced via `setrlimit()` before `exec` and inherited by child processes. The wall-clock `timeout` is enforced by the parent process — it catches builds stuck on I/O or deadlocks where CPU time does not advance.

**CPU parallelism:** Wright pins each sandbox process to its computed CPU share via `sched_setaffinity`, so `nproc` inside the sandbox already returns the correct count. Scripts should call `make -j$(nproc)` directly. To override parallelism for a specific package, set `MAKEFLAGS` (or the relevant tool variable) in `[options.env]`. See [resource-allocation.md](resource-allocation.md) for details.

**Practical guidance:** `timeout` is the most important safety net. `memory_limit` limits virtual address space (`RLIMIT_AS`), not physical RSS — set it generously (2-3x expected usage), as programs like rustc, JVM, and Go reserve large virtual mappings they never touch.

### `[lifecycle.<stage>]`

Each lifecycle stage is a TOML table under `lifecycle`:

```toml
[lifecycle.compile]
executor = "shell"
sandbox = "strict"
script = """
cd ${BUILD_DIR}
make -j$(nproc)
"""
```

| Field      | Type              | Default    | Description                            |
|------------|-------------------|------------|----------------------------------------|
| `executor` | string            | `"shell"`  | Executor to run the script with        |
| `sandbox`  | string            | `"strict"` | Sandbox isolation level                |
| `optional` | bool              | `false`    | If true, failure doesn't abort the build |
| `env`      | map of strings    | `{}`       | Extra environment variables            |
| `script`   | string            | `""`       | The script to execute                  |

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
stages = ["fetch", "verify", "extract", "configure", "compile", "package"]
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

### `[install_scripts]`

Scripts run by the package manager on the target system during install/upgrade/removal:

| Field          | Type   | Description                              |
|----------------|--------|------------------------------------------|
| `post_install` | string | Run after first install                  |
| `post_upgrade` | string | Run after upgrade                        |
| `pre_remove`   | string | Run before package removal               |

```toml
[install_scripts]
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"
```

### `[backup]`

List config files that should be preserved across upgrades:

```toml
[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

## Default Lifecycle Pipeline

The default pipeline runs these stages in order:

| Stage          | Type     | Description                              |
|----------------|----------|------------------------------------------|
| `fetch`        | built-in | Download sources and copy local files    |
| `verify`       | built-in | Verify SHA-256 checksums                 |
| `extract`      | built-in | Extract archives, copy non-archives to `${FILES_DIR}` |
| `prepare`      | user     | Pre-build setup (e.g. apply patches)     |
| `configure`    | user     | Run configure scripts                    |
| `compile`      | user     | Compile the software                     |
| `check`        | user     | Run test suites                          |
| `package`      | user     | Install files into `${PKG_DIR}`          |
| `post_package` | user     | Post-packaging steps                     |

Built-in stages (`fetch`, `verify`, `extract`) are handled by the build tool automatically. User stages are only run if defined in `plan.toml` — undefined stages are silently skipped.

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

Execution order for each stage: `pre_<stage>` → `<stage>` → `post_<stage>`. Hooks are only run if defined. They support the same fields as any lifecycle stage (`executor`, `sandbox`, `optional`, `env`, `script`).

## Phase-Based Cycles (MVP → Full)

Some packages have genuine circular build-time dependencies. The classic example is `freetype` ↔ `harfbuzz`: freetype needs harfbuzz for OpenType shaping, and harfbuzz needs freetype for glyph rendering. These cycles cannot be broken by fixing dependency types — they are real.

Wright resolves them automatically using a **two-pass build**:

1. **MVP pass** — builds the package without the cyclic dependency (functional but reduced).
2. **Full pass** — after the rest of the cycle is built, rebuilds the package with all dependencies.

### Declaring an MVP phase

Define MVP-specific dependencies so the graph becomes acyclic:

```toml
[mvp.dependencies]
link = ["freetype"] # omit harfbuzz in MVP
```

Wright's orchestrator uses Tarjan's SCC algorithm to detect cycles. If it finds a cycle and a plan in that cycle has `[mvp.dependencies]` that remove at least one edge of the cycle, it automatically inserts the two-pass schedule. If no plan provides an acyclic MVP dependency set, the build fails with a clear error identifying the cycle.

The MVP phase can also be triggered **manually** without a cycle being present, using the `--mvp` flag:

```bash
wbuild run freetype --mvp
```

This builds using `[mvp.dependencies]` and sets the same `WRIGHT_BUILD_PHASE=mvp` environment variables as an automatic cycle-breaking pass. It is useful for testing that a plan's MVP configuration is correct before it is needed in a real cycle.

### Phase environment variables

During the MVP pass, Wright injects these variables into every lifecycle stage:

| Variable | Value | Description |
|----------|-------|-------------|
| `WRIGHT_BUILD_PHASE` | `mvp` | Phase name for the MVP pass (`full` in the normal pass) |
| `WRIGHT_BOOTSTRAP_BUILD` | `1` | Set during the MVP pass for backward-compatible scripts |
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

For complex packages, it is safer to provide **dedicated MVP scripts** instead of
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

`wbuild check` validates each plan and prints a dependency graph report:

- Whether the graph is acyclic
- Each detected cycle (if any)
- MVP candidates that would break the cycle
- The selected candidate (deterministic: fewest excluded edges, then name)

### Construction Plan output

When a cycle is resolved, the plan summary shows the two-pass schedule:

```
Construction Plan:
  [MVP]           freetype
  [NEW]           harfbuzz
  [FULL]          freetype
```

`[MVP]` is the first pass (incomplete). `[FULL]` is the second pass (complete, automatically force-rebuilt). MVP builds are never written to the build cache.

When `--mvp` is used explicitly, all targets show `[MVP]` — no `[FULL]` pass follows:

```
Construction Plan:
  [MVP]           freetype
```

### Dependency type classification comes first

Most apparent cycles are caused by incorrect dependency classification. Before defining phase-specific dependencies, verify that:

- **`link`** is only used for shared libraries your binary actually links against at build time.
- **`runtime`** is used for plugins, loaders, and tools called at runtime.

For example, `gdk-pixbuf` using glycin (an image loader plugin) as a `link` dependency creates a false cycle. The correct fix is `runtime = ["glycin"]`, not a phase override.

Reserve phase-specific dependencies for cycles that remain after dependency types are correct.

## Variable Substitution

Variables use `${VAR_NAME}` syntax and are expanded in scripts and source URIs. Unrecognized variables are left as-is.

| Variable        | Description                                |
|-----------------|--------------------------------------------|
| `${PKG_NAME}`   | Package name from `[plan].name`         |
| `${PKG_VERSION}`| Package version from `[plan].version`   |
| `${PKG_RELEASE}`| Release number as a string                 |
| `${PKG_ARCH}`   | Target architecture                        |
| `${SRC_DIR}`    | Extraction root directory                  |
| `${BUILD_DIR}`  | Top-level source directory (use this in scripts) |
| `${PKG_DIR}`    | Package output directory (install files here) |
| `${FILES_DIR}`  | Directory containing non-archive files (patches, configs, etc.) |
| `${MAIN_PKG_DIR}` | Main package's output directory (only available in split package stages) |
| `${CFLAGS}`     | C compiler flags                           |
| `${CXXFLAGS}`   | C++ compiler flags                         |
| `${WRIGHT_BUILD_PHASE}` | Current phase name (`full` or `mvp`) |
| `${WRIGHT_BOOTSTRAP_BUILD}` | Set to `1` during the MVP pass (backward compatibility) |
| `${WRIGHT_BOOTSTRAP_WITHOUT_<DEP>}` | Set to `1` for each dep excluded in the MVP pass |

When running inside a sandbox, path variables are remapped to sandbox mount points:

| Variable        | Host value             | Sandbox value          |
|-----------------|------------------------|------------------------|
| `${SRC_DIR}`    | actual host path       | `/build`               |
| `${BUILD_DIR}`  | actual host path       | `/build/<source-dir>`  |
| `${PKG_DIR}`    | actual host path       | `/output`              |
| `${FILES_DIR}`  | actual host path       | `/files`               |
| `${MAIN_PKG_DIR}` | actual host path    | `/main-pkg`            |

`${BUILD_DIR}` points to the top-level directory extracted from the source archive. For example, if `nginx-1.25.3.tar.gz` extracts to `nginx-1.25.3/`, then `${BUILD_DIR}` is `${SRC_DIR}/nginx-1.25.3`. If the archive extracts files directly without a top-level directory, `${BUILD_DIR}` equals `${SRC_DIR}`. Use `${BUILD_DIR}` instead of manually `cd`-ing into the source directory.

Additionally, the following host environment variables are passed through to the build if set: `CC`, `CXX`, `AR`, `AS`, `LD`, `NM`, `RANLIB`, `STRIP`, `OBJCOPY`, `OBJDUMP`, `CFLAGS`, `CXXFLAGS`, `CPPFLAGS`, `LDFLAGS`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `LIBRARY_PATH`, `PKG_CONFIG_PATH`, `PKG_CONFIG_SYSROOT_DIR`, `MAKEFLAGS`, `JOBS`.

## Sandbox Levels

The `sandbox` field on each lifecycle stage controls process isolation:

### `none`

No isolation. The script runs directly on the host. Use this only when sandbox support is unavailable or for stages that need full host access.

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

In both `relaxed` and `strict` modes, the sandbox:
- Pivots to a minimal root filesystem
- Bind-mounts `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64` read-only
- Bind-mounts essential `/etc` files (`resolv.conf`, `hosts`, `passwd`, `group`, `ld.so.conf`, `ld.so.cache`) read-only
- Mounts the source directory at `/build` (read-write)
- Mounts the package output directory at `/output` (read-write)
- Mounts the files directory at `/files` (read-only, if present)
- Provides `/dev` with basic devices (`null`, `zero`, `urandom`, `random`, `full`)
- Mounts a fresh `/proc` and `/tmp`
- Sets hostname to `wright-sandbox`

If the kernel does not support the required namespaces (e.g. inside a container), the sandbox falls back to direct execution with a warning.

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
default_sandbox = "strict"
```

| Field              | Type            | Default      | Description                          |
|--------------------|-----------------|--------------|--------------------------------------|
| `name`             | string          | required     | Executor name used in lifecycle stages |
| `description`      | string          | `""`         | Human-readable description           |
| `command`          | string          | required     | Path to the interpreter              |
| `args`             | list of strings | `[]`         | Arguments before the script path     |
| `delivery`         | string          | `"tempfile"` | How the script is passed to the command |
| `tempfile_extension`| string         | `".sh"`      | File extension for the temp script   |
| `required_paths`   | list of strings | `[]`         | Extra paths to bind-mount in sandbox |
| `default_sandbox`  | string          | `""`         | Default sandbox level for this executor |

Reference a custom executor by name:

```toml
[lifecycle.configure]
executor = "python"
script = """
import os
os.makedirs(f"{os.environ['PKG_DIR']}/usr/lib", exist_ok=True)
"""
```

## Examples

### Minimal Plan

```toml
[plan]
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test package"
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

[lifecycle.package]
script = """
install -Dm755 hello ${PKG_DIR}/usr/bin/hello
"""
```

### Real-World Plan (nginx)

```toml
[plan]
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
optional = [
    { name = "geoip", description = "GeoIP module support" },
]
conflicts = ["apache"]
provides = ["http-server"]

[sources]
uris = [
    "https://nginx.org/download/nginx-${PKG_VERSION}.tar.gz",
    "patches/fix-headers.patch",
]
sha256 = [
    "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83",
    "SKIP",
]

[options]
strip = true
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
optional = true
script = """
cd ${BUILD_DIR}
make test
"""

[lifecycle.package]
script = """
cd ${BUILD_DIR}
make DESTDIR=${PKG_DIR} install
"""

[install_scripts]
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"

[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

### Split Packages

A single plan can produce multiple output packages. This avoids rebuilding the same source just to partition files into separate archives. Common use cases: separating documentation, libraries, or development headers from the main package.

```toml
[plan]
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[lifecycle.compile]
script = "make -j$(nproc)"

[lifecycle.package]
script = """
cd ${BUILD_DIR}
make DESTDIR=${PKG_DIR} install
rm -rf ${PKG_DIR}/usr/lib/libstdc++*
"""

[split."libstdc++"]
description = "GNU C++ standard library"

[split."libstdc++".dependencies]
runtime = ["libgcc"]

[split."libstdc++".lifecycle.package]
script = """
cd ${BUILD_DIR}
install -Dm755 libstdc++.so.6.0.33 ${PKG_DIR}/usr/lib/libstdc++.so.6.0.33
ln -sf libstdc++.so.6.0.33 ${PKG_DIR}/usr/lib/libstdc++.so.6
ln -sf libstdc++.so.6 ${PKG_DIR}/usr/lib/libstdc++.so
"""
```

Split packages inherit `version`, `release`, `arch`, and `license` from the parent `[plan]` unless overridden. Each split must have a `description` and a `[split.<name>.lifecycle.package]` stage. The shared build stages (`prepare`, `configure`, `compile`, etc.) run only once — each split's `package` stage runs afterward with its own `${PKG_DIR}`. Names containing `+` or `.` must be quoted in TOML table headers (e.g. `[split."libstdc++"]`).

The `${MAIN_PKG_DIR}` variable is available in split package stages and points to the main package's output directory. Use it to move files from the main package into the split:

```toml
[lifecycle.package]
script = "cd ${BUILD_DIR} && make DESTDIR=${PKG_DIR} install"

[split."libfoo-dev"]
description = "Development headers for libfoo"

[split."libfoo-dev".lifecycle.package]
script = """
mv ${MAIN_PKG_DIR}/usr/include ${PKG_DIR}/usr/include
mv ${MAIN_PKG_DIR}/usr/lib/pkgconfig ${PKG_DIR}/usr/lib/pkgconfig
"""
```

Split packages are independent archives — installing the parent does **not** automatically install its splits. To create a meta-package that pulls in all splits, list them as `runtime` dependencies on the parent:

```toml
[plan]
name = "linux-firmware"
# ...

[dependencies]
runtime = ["linux-firmware-amd", "linux-firmware-intel", "linux-firmware-nvidia"]

[split.linux-firmware-amd]
description = "AMD GPU/CPU firmware"
# ...
```

In this pattern the parent package itself may contain no files — it exists only to group the splits.

For a `-doc` split that overrides the architecture:

```toml
[split.mypackage-doc]
description = "Documentation for mypackage"
arch = "any"

[split.mypackage-doc.lifecycle.package]
script = """
cd ${BUILD_DIR}
install -d ${PKG_DIR}/usr/share/doc/mypackage
cp -r docs/* ${PKG_DIR}/usr/share/doc/mypackage/
"""
```

## Validation Rules

Wright validates `plan.toml` on parse. A plan that fails validation cannot be built.

| Rule | Detail |
|------|--------|
| **name** | Must match `[a-z0-9][a-z0-9_+.-]*`, max 64 characters. Names containing `+` or `.` must be quoted in TOML table headers (e.g. `[split."libstdc++"]`). |
| **version** | Any non-empty string containing alphanumeric characters (e.g. `1.25.3`, `6.5-20250809`, `2024a`) |
| **release** | Must be >= 1 |
| **description** | Must not be empty |
| **license** | Must not be empty |
| **arch** | Must not be empty |
| **sha256 count** | Must exactly match the number of `uris` entries (use `"SKIP"` for local paths) |

The output archive is named `{name}-{version}-{release}-{arch}.wright.tar.zst`.
