# Writing Plans

A **plan** is a directory containing a `package.toml` file that describes how to fetch, build, and package a piece of software. This guide is the complete reference for plan authors.

## Directory Structure

Plans live in a flat directory tree. Each plan is a directory named after the package:

```
plans/
├── hello/
│   └── package.toml
├── nginx/
│   ├── package.toml
│   └── patches/
│       └── fix-headers.patch
└── python/
    ├── package.toml
    └── patches/
        ├── 001-fix-paths.patch
        └── 002-no-rpath.patch
```

The directory name should match the `name` field in `package.toml`. Patch files referenced in `[sources].patches` are relative to the plan directory.

## `package.toml` Reference

### `[package]` — Metadata

| Field         | Type     | Required | Default | Description                        |
|---------------|----------|----------|---------|------------------------------------|
| `name`        | string   | yes      | —       | Package name                       |
| `version`     | string   | yes      | —       | Upstream version (semver)          |
| `release`     | integer  | yes      | —       | Build revision (must be >= 1)      |
| `description` | string   | yes      | —       | Short description (must not be empty) |
| `license`     | string   | yes      | —       | SPDX license identifier           |
| `arch`        | string   | yes      | —       | Target architecture (e.g. `x86_64`) |
| `url`         | string   | no       | —       | Upstream project URL               |
| `maintainer`  | string   | no       | —       | Maintainer name and email          |
| `group`       | string   | no       | —       | Package group (e.g. `core`, `extra`) |

### `[dependencies]`

All fields default to empty lists if omitted.

| Field       | Type                            | Description                          |
|-------------|---------------------------------|--------------------------------------|
| `runtime`   | list of strings                 | Required at runtime                  |
| `build`     | list of strings                 | Required only during build           |
| `optional`  | list of `{name, description}`   | Optional runtime dependencies        |
| `conflicts` | list of strings                 | Packages that conflict with this one |
| `provides`  | list of strings                 | Virtual packages this one provides   |

#### Version constraints

Runtime, build, conflicts, and provides entries can include a version constraint:

```toml
runtime = ["openssl >= 3.0", "pcre2 >= 10.42", "zlib"]
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
| `urls`    | list of strings | `[]`    | Source archive URLs                      |
| `sha256`  | list of strings | `[]`    | SHA-256 checksums (one per URL, in order) |
| `patches` | list of strings | `[]`    | Patch files relative to the plan directory |

URLs support variable substitution (see [Variable Substitution](#variable-substitution)):

```toml
urls = ["https://nginx.org/download/nginx-${version}.tar.gz"]
```

### `[options]`

| Field    | Type | Default | Description                              |
|----------|------|---------|------------------------------------------|
| `strip`  | bool | `true`  | Strip debug symbols from binaries        |
| `static` | bool | `false` | Build statically linked binaries         |
| `debug`  | bool | `false` | Build with debug info                    |
| `ccache` | bool | `true`  | Use ccache for compilation if available  |

### `[lifecycle.<stage>]`

Each lifecycle stage is a TOML table under `lifecycle`:

```toml
[lifecycle.build]
executor = "shell"
sandbox = "strict"
script = """
make -j${NPROC}
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
stages = ["fetch", "verify", "extract", "configure", "build", "package"]
```

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
| `fetch`        | built-in | Download source archives                 |
| `verify`       | built-in | Verify SHA-256 checksums                 |
| `extract`      | built-in | Extract source archives                  |
| `prepare`      | user     | Apply patches, pre-build setup           |
| `configure`    | user     | Run configure scripts                    |
| `build`        | user     | Compile the software                     |
| `check`        | user     | Run test suites                          |
| `package`      | user     | Install files into `${PKG_DIR}`          |
| `post_package` | user     | Post-packaging steps                     |

Built-in stages (`fetch`, `verify`, `extract`) are handled by the build tool automatically. User stages are only run if defined in `package.toml` — undefined stages are silently skipped.

Override this order with `[lifecycle_order]` if your build needs a different pipeline.

## Pre/Post Hooks

Any stage can have a pre- or post-hook. Name them `pre_<stage>` or `post_<stage>`:

```toml
[lifecycle.pre_build]
script = """
echo "About to build..."
"""

[lifecycle.build]
script = """
make -j${NPROC}
"""

[lifecycle.post_build]
script = """
echo "Build complete."
"""
```

Execution order for each stage: `pre_<stage>` → `<stage>` → `post_<stage>`. Hooks are only run if defined. They support the same fields as any lifecycle stage (`executor`, `sandbox`, `optional`, `env`, `script`).

## Variable Substitution

Variables use `${VAR_NAME}` syntax and are expanded in scripts and source URLs. Unrecognized variables are left as-is.

| Variable        | Description                                |
|-----------------|--------------------------------------------|
| `${PKG_NAME}`   | Package name from `[package].name`         |
| `${PKG_VERSION}`| Package version from `[package].version`   |
| `${PKG_RELEASE}`| Release number as a string                 |
| `${PKG_ARCH}`   | Target architecture                        |
| `${SRC_DIR}`    | Source/working directory                   |
| `${PKG_DIR}`    | Package output directory (install files here) |
| `${PATCHES_DIR}`| Directory containing patch files           |
| `${NPROC}`      | Number of available CPUs                   |
| `${CFLAGS}`     | C compiler flags                           |
| `${CXXFLAGS}`   | C++ compiler flags                         |
| `${version}`    | Alias for `${PKG_VERSION}` (for use in URLs) |

When running inside a sandbox, path variables are remapped to sandbox mount points:

| Variable        | Host value             | Sandbox value |
|-----------------|------------------------|---------------|
| `${SRC_DIR}`    | actual host path       | `/build`      |
| `${PKG_DIR}`    | actual host path       | `/output`     |
| `${PATCHES_DIR}`| actual host path       | `/patches`    |

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
- Mounts the patches directory at `/patches` (read-only, if present)
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
[package]
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

[lifecycle.build]
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
[package]
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"
arch = "x86_64"
url = "https://nginx.org"
maintainer = "Example Maintainer <maintainer@example.com>"
group = "extra"

[dependencies]
runtime = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]
build = ["perl", "gcc", "make"]
optional = [
    { name = "geoip", description = "GeoIP module support" },
]
conflicts = ["apache"]
provides = ["http-server"]

[sources]
urls = ["https://nginx.org/download/nginx-${version}.tar.gz"]
sha256 = ["a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"]
patches = ["patches/fix-headers.patch"]

[options]
strip = true
static = false
debug = false
ccache = true

[lifecycle.prepare]
script = """
cd nginx-${PKG_VERSION}
for p in ${PATCHES_DIR}/*.patch; do
    [ -f "$p" ] && patch -p1 < "$p"
done
"""

[lifecycle.configure]
env = { CFLAGS = "-O2 -pipe" }
script = """
cd nginx-${PKG_VERSION}
./configure --prefix=/usr
"""

[lifecycle.build]
script = """
cd nginx-${PKG_VERSION}
make -j${NPROC}
"""

[lifecycle.check]
optional = true
script = """
cd nginx-${PKG_VERSION}
make test
"""

[lifecycle.package]
script = """
cd nginx-${PKG_VERSION}
make DESTDIR=${PKG_DIR} install
"""

[install_scripts]
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"

[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

## Validation Rules

Wright validates `package.toml` on parse. A plan that fails validation cannot be built.

| Rule | Detail |
|------|--------|
| **name** | Must match `[a-z0-9][a-z0-9_-]*`, max 64 characters |
| **version** | Must be valid semver: `MAJOR`, `MAJOR.MINOR`, or `MAJOR.MINOR.PATCH` (each component is a non-negative integer) |
| **release** | Must be >= 1 |
| **description** | Must not be empty |
| **license** | Must not be empty |
| **arch** | Must not be empty |
| **sha256 count** | Must exactly match the number of `urls` entries |

The output archive is named `{name}-{version}-{release}-{arch}.wright.tar.zst`.
