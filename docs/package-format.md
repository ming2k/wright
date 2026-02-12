# Package Format

Packages are described by `package.toml` files in the hold tree. Each hold is a directory containing at minimum a `package.toml` and optionally a `patches/` subdirectory.

## Complete package.toml Specification

### `[package]` — metadata (required)

```toml
[package]
name = "nginx"                          # [a-z0-9][a-z0-9_-]*, max 64 chars
version = "1.25.3"                      # Semantic version
release = 1                             # Integer >= 1, increment on build script changes
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"                # SPDX identifier
arch = "x86_64"                         # Target architecture, or "any"
url = "https://nginx.org"               # Upstream homepage (optional)
maintainer = "Name <email>"             # Package maintainer (optional)
group = "extra"                         # Tier: core / base / extra / community (optional)
```

| Field | Required | Validation |
|-------|----------|------------|
| `name` | yes | Must match `[a-z0-9][a-z0-9_-]*`, max 64 chars |
| `version` | yes | Semantic version (e.g., `1.0.0`, `2.1.3`) |
| `release` | yes | Integer >= 1 |
| `description` | yes | Non-empty string |
| `license` | yes | Non-empty string (SPDX identifier recommended) |
| `arch` | yes | Non-empty string |
| `url` | no | Upstream project URL |
| `maintainer` | no | Maintainer name and email |
| `group` | no | Package tier |

### `[dependencies]` — dependency declarations

```toml
[dependencies]
runtime = [
    "openssl",                  # Any version
    "pcre2 >= 10.42",          # Version constraint
    "zlib >= 1.2",
]
build = ["perl", "gcc", "make"]
optional = [
    { name = "geoip", description = "GeoIP module support" },
]
conflicts = ["apache"]
provides = ["http-server"]
```

| Field | Type | Description |
|-------|------|-------------|
| `runtime` | string[] | Required at runtime. Version constraints: `>=`, `<=`, `=`, `>`, `<` |
| `build` | string[] | Required only at build time |
| `optional` | object[] | Optional features (name + description) |
| `conflicts` | string[] | Cannot coexist with these packages |
| `provides` | string[] | Virtual packages this package satisfies |

### `[sources]` — source definitions

```toml
[sources]
urls = [
    "https://nginx.org/download/nginx-${version}.tar.gz",
]
sha256 = [
    "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83",
]
patches = [
    "patches/fix-headers.patch",
]
```

| Field | Type | Description |
|-------|------|-------------|
| `urls` | string[] | Source archive URLs (supports `${version}` substitution) |
| `sha256` | string[] | SHA-256 checksums, one per URL (must match count) |
| `patches` | string[] | Patch files relative to the hold directory |

### `[options]` — build options

```toml
[options]
strip = true        # Strip binaries (default: true)
static = false      # Static linking (default: false)
debug = false       # Preserve debug symbols (default: false)
ccache = true       # Enable ccache (default: true)
```

### `[lifecycle.<stage>]` — build stages

Each lifecycle stage defines a script to run:

```toml
[lifecycle.build]
executor = "shell"              # Executor name (default: "shell")
sandbox = "strict"              # Sandbox level (default: "strict")
optional = false                # Whether failure is non-fatal (default: false)
env = { MAKEFLAGS = "-j${NPROC}" }  # Extra environment variables
script = """
cd nginx-${PKG_VERSION}
make
"""
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `executor` | string | `"shell"` | Which executor runs this script |
| `sandbox` | string | `"strict"` | Isolation level: `none`, `relaxed`, `strict` |
| `optional` | boolean | `false` | If true, stage failure doesn't abort the build |
| `env` | map | `{}` | Additional environment variables |
| `script` | string | `""` | Script content to execute |

### Default pipeline order

```
fetch → verify → extract → prepare → configure → build → check → package → post_package
```

- **fetch**, **verify**, **extract** are handled automatically by the build tool
- Stages without a defined script are skipped
- Each stage supports **pre/post hooks** (e.g., `pre_build`, `post_build`)

### Custom pipeline order

Override the default stage ordering:

```toml
[lifecycle_order]
stages = ["fetch", "verify", "extract", "prepare", "codegen", "configure", "build", "package"]
```

### `[install_scripts]` — post-install hooks

Scripts that run on the real system (not sandboxed) after installation:

```toml
[install_scripts]
post_install = """
useradd -r -s /sbin/nologin nginx 2>/dev/null || true
"""
post_upgrade = """
sv restart nginx 2>/dev/null || true
"""
pre_remove = """
sv stop nginx 2>/dev/null || true
"""
```

### `[backup]` — preserved config files

Configuration files that are preserved on uninstall and not overwritten on upgrade:

```toml
[backup]
files = [
    "/etc/nginx/nginx.conf",
    "/etc/nginx/mime.types",
]
```

## Variable Substitution

Script fields support these variables, expanded before execution:

| Variable | Description |
|----------|-------------|
| `${PKG_NAME}` | Package name |
| `${PKG_VERSION}` | Package version |
| `${PKG_RELEASE}` | Release number |
| `${PKG_ARCH}` | Target architecture |
| `${SRC_DIR}` | Source directory (build working directory) |
| `${PKG_DIR}` | Package output directory (simulated install root) |
| `${PATCHES_DIR}` | Patch files directory |
| `${NPROC}` | Number of CPU cores |
| `${CFLAGS}` | Global C compiler flags |
| `${CXXFLAGS}` | Global C++ compiler flags |

Variables use the `${VAR}` syntax. They are substituted in `script` fields and `env` values.

## Sandbox Levels

| Level | Namespaces | Network | Description |
|-------|-----------|---------|-------------|
| `none` | None | Allowed | No isolation, direct execution |
| `relaxed` | Mount, PID, User | Allowed | Basic isolation, network access permitted |
| `strict` | Mount, PID, Net, User, IPC, UTS | Blocked | Full isolation, no network access |

## Binary Package Format

Built packages are tar.zst archives with the naming convention:

```
{name}-{version}-{release}-{arch}.wright.tar.zst
```

Internal structure:

```
├── .PKGINFO        # Package metadata (TOML)
├── .FILELIST       # File manifest (one path per line)
├── .INSTALL        # Install scripts (if any)
└── usr/            # Installed files
    ├── bin/
    ├── lib/
    ├── share/
    └── ...
```

## Minimal Example

```toml
[package]
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World"
license = "MIT"
arch = "x86_64"

[lifecycle.prepare]
executor = "shell"
sandbox = "none"
script = """
cat > hello.c << 'CEOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
CEOF
"""

[lifecycle.build]
executor = "shell"
sandbox = "none"
script = "gcc -o hello hello.c"

[lifecycle.package]
executor = "shell"
sandbox = "none"
script = "install -Dm755 hello ${PKG_DIR}/usr/bin/hello"
```

## Full Example

See the nginx example in [design-spec.md](design-spec.md#4-package-description-format-packagetoml) for a comprehensive, real-world package description.
