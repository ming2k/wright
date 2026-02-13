# wright — Declarative, Extensible, Sandboxed Linux Package Manager Technical Design Specification

> This document is a comprehensive technical design specification intended to guide AI agents or developers in implementing a modern package management system from scratch for LFS (Linux From Scratch) based distributions.

---

## 1. Project Overview

### 1.1 Project Name

`wright` — A declarative, extensible, sandboxed Linux package management and build system.

### 1.2 Project Goals

Provide a complete package management solution for custom Linux distributions built on LFS, with the following core features:

- **Declarative package descriptions**: Define package metadata, dependencies, sources, and build procedures using TOML format
- **Lifecycle pipeline**: Build processes are split into ordered stages (lifecycle stages), each supporting pre/post hooks
- **Pluggable executors**: Build scripts are not limited to shell — support Python, Lua, and other runtimes
- **Sandbox isolation**: All build stages run in Linux namespace-isolated environments using bubblewrap (bwrap)
- **Transactional operations**: Package installation and removal support atomic operations with rollback
- **Binary package distribution**: Build artifacts are distributable binary packages (tar.zst format)

### 1.3 Target Users

Developers building custom, long-term maintainable Linux distributions from LFS for personal or small-team use.

### 1.4 Design Philosophy

- Borrow Nix's reproducibility concepts without adopting its full functional model
- Borrow pacman/XBPS's simplicity and efficiency
- Borrow hold system structure from BSD-like systems
- Keep implementation simple and avoid over-engineering

### 1.5 Distribution Philosophy

The wright package manager is designed to serve a distribution with a clear, opinionated identity. The following principles define the character of the target distribution and **must** inform all packaging decisions, default configurations, and repository policies.

#### Core Principles

1. **Simplicity over abstraction**: Prefer tools that are transparent, understandable, and debuggable. Avoid black-box systems with hidden complexity. A user should be able to read the source, understand the boot process, and trace any system behavior without specialized knowledge.

2. **Standards compliance**: Adhere to POSIX, FHS (Filesystem Hierarchy Standard), and established Linux conventions wherever possible. Avoid reinventing interfaces that already have well-defined standards.

3. **Stability over bleeding-edge**: Prefer well-tested, proven software. The repository should track stable upstream releases, not development branches. Security patches are always prioritized.

4. **Minimal base, explicit choice**: The base system should be as small as practical. Additional functionality is added explicitly by the user, never assumed. No pre-installed bloat.

5. **Low black-box factor**: Every component in the system should be auditable and replaceable. Prefer software with small codebases, clear documentation, and predictable behavior over feature-rich but opaque alternatives.

#### System Component Choices

The following choices reflect the distribution's philosophy and **must** be treated as architectural decisions, not suggestions:

| Layer | Choice | Rationale |
|-------|--------|-----------|
| **C library** | **musl libc** | Small, correct, standards-compliant. Static linking friendly. Avoids glibc's complexity and legacy baggage. Some software may need patches for musl compatibility — this is acceptable and expected. |
| **Init system** | **runit** | Simple, reliable, easy to understand. Service scripts are plain shell, not a DSL. The entire init system is auditable in an afternoon. No socket activation, no dependency graphs, no DBus requirement. |
| **Core utilities** | **busybox + selective GNU** | Busybox for the base, with GNU coreutils/findutils/grep/sed available as optional packages for users who need full GNU compatibility. |
| **Shell** | **bash** (default build/user shell) | Widely compatible, well-understood. `/bin/sh` may symlink to busybox ash or dash for scripting performance. |
| **TLS/SSL** | **LibreSSL** or **OpenSSL** | Either is acceptable. LibreSSL preferred for its cleaner codebase if compatibility allows; OpenSSL as fallback for maximum compatibility. |
| **Desktop application isolation** | **Flatpak** | Third-party GUI applications (browsers, office suites, etc.) should be distributed via Flatpak rather than packaged natively. This keeps the native repository focused on system packages, libraries, and CLI tools. Flatpak provides its own runtime and sandboxing, reducing maintenance burden for complex desktop applications. |
| **Compiler toolchain** | **GCC** (primary), **LLVM/Clang** (optional) | GCC as the default system compiler for maximum compatibility. Clang available as an alternative. |

#### Repository Tiers

The hold tree and binary repository are organized into tiers with different stability guarantees:

| Tier | Name | Description | Update Policy |
|------|------|-------------|---------------|
| **core** | Core System | Toolchain, libc, kernel, init, essential utilities. Minimal set required to boot and build packages. | Conservative. Updates only for security fixes and critical bugs. Major version bumps require explicit migration. |
| **base** | Base System | Networking, filesystem tools, common libraries, package manager itself. Expected on most installations. | Stable releases only. Tested against core before promotion. |
| **extra** | Extra Packages | Servers, languages, development tools, libraries. General-purpose software. | Stable upstream releases. Reasonable testing before inclusion. |
| **community** | Community | User-contributed packages. Lower barrier to entry. | Maintained by contributors. No stability guarantee from the distribution. |

#### What the Native Repository Should NOT Contain

- **Desktop applications with complex dependency trees**: Use Flatpak instead (browsers, LibreOffice, Electron apps, etc.)
- **Multiple versions of the same library**: The repository tracks one version per package. If parallel installation is needed, the user manages it manually or uses containers.
- **Abandoned or unmaintained upstream software**: Packages must have active upstream maintenance or a clear fork/replacement path.
- **Proprietary software**: The native repository is free/open-source only. Proprietary software can be installed via Flatpak or user-managed methods.

#### musl Compatibility Policy

Since musl libc is the system C library, some upstream software will require patches or configuration changes. The policy is:

- **Patches are acceptable and expected**: Maintain musl compatibility patches in the hold tree. Upstream contributions are encouraged.
- **If a package cannot be reasonably patched for musl**: It should not be in the native repository. Suggest Flatpak (which uses its own glibc runtime) or a glibc chroot as alternatives.
- **Common musl issues to handle**: Missing `execinfo.h` (backtrace), `sys/cdefs.h` differences, locale limitations, `GLOB_TILDE`/`GLOB_BRACE` unavailability, `wordexp` differences. The build system should provide common musl compatibility flags and patches as reusable components.

#### runit Service Convention

All packages that provide system services must include runit service directories following this structure:

```
/etc/sv/<service_name>/
├── run              # Service run script (executable)
├── finish           # Optional cleanup script (executable)
└── log/
    └── run          # Logging run script (pipes to svlogd)
```

Example `run` script:
```bash
#!/bin/sh
exec chpst -u nginx nginx -g 'daemon off;'
```

Example `log/run` script:
```bash
#!/bin/sh
exec svlogd -tt /var/log/<service_name>
```

Services are **not** enabled by default. The user explicitly enables services by symlinking into `/var/service/`.

---

## 2. Technology Stack

### 2.1 Core Development Language

**Rust (latest stable)**

Rationale:
- Performance comparable to C, suitable for system tools
- Memory safety without garbage collection
- Strong type system ideal for modeling complex package management state
- First-class TOML parsing support via `serde` ecosystem
- Excellent CLI tooling ecosystem (`clap`, `indicatif`, etc.)

### 2.2 Key Dependencies

| Purpose | Crate | Notes |
|---------|-------|-------|
| TOML parsing | `toml` / `serde` | Deserialize package description files |
| CLI framework | `clap` (derive API) | Command-line argument parsing |
| Filesystem operations | `walkdir`, `tempfile` | Directory traversal, temporary build directories |
| Compression/archiving | `tar`, `zstd` | Binary package format (tar.zst) |
| Hash verification | `sha2` | Source integrity verification (SHA-256) |
| HTTP downloads | `reqwest` (blocking) | Source code downloads |
| Process management | `std::process::Command` | Invoke bubblewrap and executors |
| Database | `rusqlite` (SQLite) | Local installed package database |
| Dependency resolution | Custom topological sort | No SAT solver needed initially |
| Progress display | `indicatif` | Download and build progress bars |
| Logging | `tracing` | Structured logging |
| Error handling | `anyhow` / `thiserror` | Application-level and library-level error handling |

### 2.3 External Tool Dependencies

| Tool | Purpose | Required |
|------|---------|----------|
| `bubblewrap` (bwrap) | Sandbox isolation (namespaces) | **Yes** |
| `bash` | Default shell executor | **Yes** |
| `python3` | Python executor | Optional |
| `curl` / `wget` | Fallback download tools | Optional |
| `gpg` | Package signature verification | Optional (later phase) |

### 2.4 Package Format

- Binary package format: **tar.zst** (zstd-compressed tar archive)
- Package file extension: `.wright.tar.zst`
- Package description format: **TOML**
- Package filename convention: `{name}-{version}-{release}-{arch}.wright.tar.zst`

---

## 3. System Architecture

### 3.1 Binary Components

The system consists of three main binaries:

```
wright           # Package manager (install, remove, query, upgrade)
wright-build     # Build tool (parse build descriptions, execute builds, create packages)
wright-repo      # Repository tool (generate index, sign, verify)
```

### 3.2 Directory Layout

```
/etc/wright/
├── wright.toml                   # Global configuration
├── repos.toml                  # Repository source configuration
└── executors/                  # Executor definitions
    ├── shell.toml
    ├── python.toml
    └── lua.toml

/var/lib/wright/
├── db/
│   └── packages.db             # SQLite database (installed package info)
├── cache/
│   ├── sources/                # Downloaded source code cache
│   └── packages/               # Downloaded/built binary package cache
└── lock/                       # Global lock file (prevent concurrent operations)

/var/hold/                      # Hold tree (collection of build description files)
├── core/                       # Core system packages
│   ├── glibc/
│   │   └── package.toml
│   ├── gcc/
│   │   ├── package.toml
│   │   └── patches/
│   └── openssl/
│       └── package.toml
├── extra/                      # Additional packages
│   ├── nginx/
│   │   └── package.toml
│   └── python/
│       └── package.toml
└── custom/                     # User-defined packages
```

### 3.3 Architecture Layers

```
┌─────────────────────────────────────────────────────┐
│                    CLI Interface                      │
│                (wright / wright-build)                    │
├─────────────────────────────────────────────────────┤
│                   Core Logic Layer                    │
│  ┌──────────────┬──────────────┬──────────────────┐ │
│  │   Resolver    │  Transaction │     Builder      │ │
│  │ (dependency   │  (atomic     │  (build          │ │
│  │  resolution)  │   ops)       │   pipeline)      │ │
│  └──────────────┴──────────────┴──────────────────┘ │
├─────────────────────────────────────────────────────┤
│                  Subsystem Layer                      │
│  ┌──────────┬──────────┬──────────┬───────────────┐ │
│  │ Database │ Sandbox  │ Executor │  Downloader    │ │
│  │ (SQLite) │ (bwrap)  │ (plugin) │  (reqwest)     │ │
│  └──────────┴──────────┴──────────┴───────────────┘ │
├─────────────────────────────────────────────────────┤
│                System Interface Layer                 │
│       (filesystem, namespace, process, network)      │
└─────────────────────────────────────────────────────┘
```

---

## 4. Package Description Format (package.toml)

### 4.1 Complete Field Specification

```toml
# ==============================================================
# package.toml — wright package description file full specification
# ==============================================================

# ---- Package metadata (required) ----
[package]
name = "nginx"                          # Package name, [a-z0-9][a-z0-9_-]*, max 64 chars
version = "1.25.3"                      # Semantic version (semver format)
release = 1                             # Release number (integer, increment when build script changes)
description = "High performance HTTP and reverse proxy server"
license = "BSD-2-Clause"                # SPDX license identifier
arch = "x86_64"                         # Target architecture, or "any" for arch-independent
url = "https://nginx.org"               # Upstream project homepage (optional)
maintainer = "Your Name <you@email>"    # Maintainer (optional)
group = "extra"                         # Group: core / extra / custom (optional)

# ---- Dependency declarations ----
[dependencies]
# Runtime dependencies: must be installed when this package is installed
runtime = [
    "openssl",              # Simple dependency (any version)
    "pcre2 >= 10.42",       # Version constraint: >=, <=, =, >, <
    "zlib >= 1.2",
]

# Build dependencies: needed only at build time, not recorded as runtime deps
build = ["perl", "gcc", "make"]

# Optional dependencies: provide extra functionality, not enforced
optional = [
    { name = "geoip", description = "GeoIP module support" },
]

# Conflicts: cannot coexist with these packages
conflicts = ["apache"]

# Provides: this package can substitute for these virtual packages
provides = ["http-server"]

# ---- Source definitions ----
[sources]
# Primary source archive
urls = [
    "https://nginx.org/download/nginx-${version}.tar.gz",
]

# SHA-256 checksum for each URL (order corresponds to urls)
sha256 = [
    "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83",
]

# Local patch files (relative to the package.toml directory)
patches = [
    "patches/fix-headers.patch",
    "patches/add-feature.patch",
]

# ---- Build options ----
[options]
strip = true            # Strip binaries (default: true)
static = false          # Static linking (default: false)
debug = false           # Preserve debug symbols (default: false)
ccache = true           # Enable ccache if available (default: true)

# ---- Lifecycle definitions ----
# Default pipeline order:
#   fetch → verify → extract → prepare → configure → build → check → package → post_package
#
# Each stage format:
#   [lifecycle.<stage_name>]
#   executor = "shell"           # Executor name (matches definition in /etc/wright/executors/)
#   sandbox = "strict"           # Sandbox level: none / relaxed / strict
#   optional = false             # Whether failure is non-fatal
#   env = { KEY = "VALUE" }      # Additional environment variables
#   script = """..."""           # Execution content
#
# Note: fetch / verify / extract stages are handled automatically by the build tool.
# They typically do not need manual definition unless special behavior is required.

[lifecycle.prepare]
executor = "shell"
sandbox = "strict"
script = """
cd ${BUILD_DIR}
for p in ${PATCHES_DIR}/*.patch; do
    [ -f "$p" ] && patch -p1 < "$p"
done
"""

[lifecycle.configure]
executor = "shell"
sandbox = "strict"
env = { CFLAGS = "-O2 -pipe -march=x86-64", CXXFLAGS = "${CFLAGS}" }
script = """
cd ${BUILD_DIR}
./configure \
    --prefix=/usr \
    --sysconfdir=/etc/nginx \
    --with-http_ssl_module \
    --with-http_v2_module \
    --with-pcre-jit
"""

[lifecycle.build]
executor = "shell"
sandbox = "strict"
env = { MAKEFLAGS = "-j${NPROC}" }
script = """
cd ${BUILD_DIR}
make
"""

[lifecycle.check]
executor = "shell"
sandbox = "strict"
optional = true
script = """
cd ${BUILD_DIR}
make test
"""

[lifecycle.package]
executor = "shell"
sandbox = "strict"
script = """
cd ${BUILD_DIR}
make DESTDIR=${PKG_DIR} install
# Install configuration files
install -Dm644 conf/nginx.conf ${PKG_DIR}/etc/nginx/nginx.conf
"""

# ---- Custom stage example ----
[lifecycle.post_package]
executor = "python"
sandbox = "strict"
script = """
import os, glob

pkg_dir = os.environ['PKG_DIR']

# Remove .la files
for la in glob.glob(os.path.join(pkg_dir, '**/*.la'), recursive=True):
    os.remove(la)
    print(f"Removed: {la}")

# Remove empty directories
for root, dirs, files in os.walk(pkg_dir, topdown=False):
    for d in dirs:
        path = os.path.join(root, d)
        if not os.listdir(path):
            os.rmdir(path)
"""

# ---- Install scripts (optional) ----
# Scripts executed after the package is installed to the real system (NOT sandboxed)
[install_scripts]
post_install = """
# Create nginx user
useradd -r -s /sbin/nologin -d /var/lib/nginx nginx 2>/dev/null || true
"""

post_upgrade = """
# Reload configuration
systemctl reload nginx 2>/dev/null || true
"""

pre_remove = """
systemctl stop nginx 2>/dev/null || true
"""

# ---- Backup files ----
# Configuration files preserved on uninstall and not overwritten on upgrade
[backup]
files = [
    "/etc/nginx/nginx.conf",
    "/etc/nginx/mime.types",
]

# ---- Custom lifecycle order (optional, overrides default) ----
# [lifecycle_order]
# stages = ["fetch", "verify", "extract", "prepare", "codegen", "configure", "build", "check", "package", "post_package"]
```

### 4.2 Variable Substitution Rules

The `script` fields in package description files support the following variables (expanded by the build tool before execution):

| Variable | Description |
|----------|-------------|
| `${PKG_NAME}` | Package name |
| `${PKG_VERSION}` | Package version |
| `${PKG_RELEASE}` | Release number |
| `${PKG_ARCH}` | Target architecture |
| `${SRC_DIR}` | Source directory after extraction (build working directory) |
| `${PKG_DIR}` | Package output directory (simulated install root) |
| `${PATCHES_DIR}` | Patch files directory |
| `${NPROC}` | Number of CPU cores (for parallel compilation) |
| `${CFLAGS}` | Global C compiler flags (from config or env override) |
| `${CXXFLAGS}` | Global C++ compiler flags |

---

## 5. Executor System

### 5.1 Executor Definition Format

Each executor is a TOML file placed in `/etc/wright/executors/`:

```toml
# /etc/wright/executors/shell.toml
[executor]
name = "shell"
description = "Bash shell executor"
command = "/bin/bash"
args = ["-e", "-o", "pipefail"]     # -e: exit on error, -o pipefail: propagate pipe errors
delivery = "tempfile"               # "tempfile" or "stdin"
tempfile_extension = ".sh"          # Temporary file extension

# Additional paths required to be visible inside the sandbox (read-only)
required_paths = ["/bin", "/usr/bin"]

# Default sandbox level if not specified by the package
default_sandbox = "strict"
```

```toml
# /etc/wright/executors/python.toml
[executor]
name = "python"
description = "Python 3 executor"
command = "/usr/bin/python3"
args = ["-u"]                       # Unbuffered output
delivery = "tempfile"
tempfile_extension = ".py"
required_paths = ["/usr/lib/python3", "/usr/lib/python3.*/"]
default_sandbox = "strict"
```

```toml
# /etc/wright/executors/lua.toml
[executor]
name = "lua"
description = "Lua 5.4 executor"
command = "/usr/bin/lua"
args = []
delivery = "tempfile"
tempfile_extension = ".lua"
required_paths = ["/usr/lib/lua", "/usr/share/lua"]
default_sandbox = "strict"
```

### 5.2 Executor Invocation Flow

```
1. Read the executor field from the lifecycle stage definition
2. Load the corresponding executor definition TOML
3. Perform variable substitution on the script content
4. Based on the delivery method:
   - tempfile: Write script to a temporary file, pass path as argument to command
   - stdin: Pipe script content to command via standard input
5. Set environment variables (global + stage-level env overrides)
6. Launch the process inside the sandbox environment
7. Capture stdout/stderr, write to build log
8. Check exit code; if non-zero, decide whether to abort based on the optional field
```

### 5.3 Custom Executors

Users can add custom executors by placing new TOML files in `/etc/wright/executors/`. The build tool scans and registers all available executors at startup.

Security constraints:
- The executor `command` must be an absolute path
- The binary pointed to by `command` must exist and be executable
- Executor definitions must not contain shell metacharacters or pipes

---

## 6. Sandbox Isolation System

### 6.1 Isolation Level Definitions

| Level | Mount NS | PID NS | Network NS | User NS | IPC NS | UTS NS | seccomp |
|-------|----------|--------|-----------|---------|--------|--------|---------|
| `none` | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `relaxed` | ✅ | ✅ | ❌ | ✅ | ❌ | ❌ | ❌ |
| `strict` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ (optional) |

### 6.2 Sandbox Root Filesystem Layout

For each lifecycle stage execution, the filesystem view inside the sandbox is:

```
/ (sandbox root)
├── usr/            ← Read-only bind mount of host /usr
├── lib/            ← Read-only bind mount of host /lib (if exists)
├── lib64/          ← Read-only bind mount of host /lib64 (if exists)
├── bin/            ← Read-only bind mount of host /bin
├── sbin/           ← Read-only bind mount of host /sbin
├── etc/            ← Minimal /etc (only essentials like ld.so.conf, passwd)
├── dev/            ← Minimal devtmpfs (null, zero, urandom, full, tty)
├── proc/           ← Mounted proc
├── tmp/            ← tmpfs (build temporary files)
├── build/          ← Read-write, source directory (SRC_DIR)
├── output/         ← Read-write, package output directory (PKG_DIR)
├── patches/        ← Read-only bind mount of patches directory
└── deps/           ← Read-only bind mount of build dependencies
    ├── openssl/    ← Dependency package installed files
    └── pcre2/
```

### 6.3 bwrap Invocation Template

The build tool should generate the corresponding `bwrap` command line based on the sandbox level:

```bash
# strict mode example
bwrap \
    --ro-bind /usr /usr \
    --ro-bind /bin /bin \
    --ro-bind /sbin /sbin \
    --ro-bind /lib /lib \
    --ro-bind /lib64 /lib64 \
    --ro-bind /etc/ld.so.conf /etc/ld.so.conf \
    --ro-bind /etc/ld.so.cache /etc/ld.so.cache \
    --bind "${SRC_DIR}" /build \
    --bind "${PKG_DIR}" /output \
    --ro-bind "${PATCHES_DIR}" /patches \
    --dev /dev \
    --proc /proc \
    --tmpfs /tmp \
    --unshare-user \
    --unshare-pid \
    --unshare-net \
    --unshare-ipc \
    --unshare-uts \
    --uid 1000 \
    --gid 1000 \
    --die-with-parent \
    --chdir /build \
    --setenv PKG_NAME "${PKG_NAME}" \
    --setenv PKG_VERSION "${PKG_VERSION}" \
    --setenv PKG_DIR "/output" \
    --setenv SRC_DIR "/build" \
    --setenv PATCHES_DIR "/patches" \
    --setenv NPROC "$(nproc)" \
    -- /bin/bash -e -o pipefail /tmp/_build_script.sh
```

### 6.4 Sandbox Exceptions for Special Stages

- `fetch` stage: **Does not enter sandbox** — handled directly by the build tool using reqwest
- `verify` stage: **Does not enter sandbox** — SHA-256 verification handled directly by the build tool
- `extract` stage: **Does not enter sandbox** — extraction handled directly by the build tool
- `install_scripts` (post_install, etc.): **Does not run in sandbox** — needs to modify the real system

---

## 7. Lifecycle Pipeline

### 7.1 Default Pipeline

```
fetch → verify → extract → prepare → configure → build → check → package → post_package
```

### 7.2 Stage Descriptions

| Stage | Executed By | Sandboxed | Description |
|-------|------------|-----------|-------------|
| `fetch` | Build tool | No | Download source code and patches |
| `verify` | Build tool | No | SHA-256 checksum verification |
| `extract` | Build tool | No | Extract source archive to SRC_DIR |
| `prepare` | User script | Yes | Apply patches, preprocessing |
| `configure` | User script | Yes | ./configure and similar configuration steps |
| `build` | User script | Yes | Compilation (make, etc.) |
| `check` | User script | Yes | Run tests (optional stage) |
| `package` | User script | Yes | make install to PKG_DIR |
| `post_package` | User script | Yes | Cleanup, metadata generation, etc. |

### 7.3 Hook System

Each stage supports pre/post hooks, defined as:

```toml
[lifecycle.pre_build]
executor = "shell"
sandbox = "strict"
script = """
echo "About to start building..."
# Pre-checks or preparation work
"""

[lifecycle.build]
executor = "shell"
sandbox = "strict"
script = """
make
"""

[lifecycle.post_build]
executor = "python"
sandbox = "strict"
script = """
# Post-build automated checks
import subprocess
result = subprocess.run(['file', 'nginx'], capture_output=True, text=True)
print(f"Binary type: {result.stdout}")
"""
```

Execution order: `pre_<stage>` → `<stage>` → `post_<stage>`

### 7.4 Custom Pipelines

Packages can override the default pipeline via `[lifecycle_order]`:

```toml
[lifecycle_order]
stages = ["fetch", "verify", "extract", "prepare", "codegen", "configure", "build", "package"]
```

Stages without a defined script are automatically skipped (except fetch/verify/extract, which are handled internally by the build tool).

---

## 8. Package Manager (wright)

### 8.1 Command-Line Interface

```
wright install <pkg> [pkg...]       # Install packages (from repo or local file)
wright remove <pkg> [pkg...]        # Uninstall packages
wright upgrade [pkg...]             # Upgrade packages (no args = upgrade all)
wright query <pkg>                  # Query package information
wright list [--installed]           # List packages
wright search <keyword>             # Search packages
wright files <pkg>                  # List files owned by a package
wright owner <file>                 # Query which package owns a file
wright verify [pkg]                 # Verify package file integrity
wright rollback                     # Rollback the last operation
wright sync                         # Sync remote repository index
wright clean                        # Clean caches
```

### 8.2 SQLite Database Schema

```sql
-- Installed package information
CREATE TABLE packages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    version TEXT NOT NULL,
    release INTEGER NOT NULL,
    description TEXT,
    arch TEXT NOT NULL,
    license TEXT,
    url TEXT,
    installed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    install_size INTEGER,          -- Disk usage after installation (bytes)
    pkg_hash TEXT                   -- SHA-256 of the binary package
);

-- Package file manifest
CREATE TABLE files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id INTEGER NOT NULL,
    path TEXT NOT NULL,             -- Absolute file path
    file_hash TEXT,                 -- File SHA-256 (for verification)
    file_type TEXT NOT NULL,        -- 'file', 'dir', 'symlink'
    file_mode INTEGER,             -- Permission bits
    file_size INTEGER,             -- File size
    is_config BOOLEAN DEFAULT 0,   -- Whether this is a config file (preserved on upgrade)
    FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
);

-- Runtime dependency relationships
CREATE TABLE dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id INTEGER NOT NULL,
    depends_on TEXT NOT NULL,       -- Dependency package name
    version_constraint TEXT,        -- Version constraint string
    FOREIGN KEY (package_id) REFERENCES packages(id) ON DELETE CASCADE
);

-- Operation log (for rollback support)
CREATE TABLE transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    operation TEXT NOT NULL,        -- 'install', 'remove', 'upgrade'
    package_name TEXT NOT NULL,
    old_version TEXT,               -- Pre-upgrade version (upgrade only)
    new_version TEXT,               -- Post-install/upgrade version
    status TEXT NOT NULL,           -- 'pending', 'completed', 'rolled_back'
    backup_path TEXT                -- Rollback backup path
);

-- Indexes
CREATE INDEX idx_files_path ON files(path);
CREATE INDEX idx_files_package ON files(package_id);
CREATE INDEX idx_deps_package ON dependencies(package_id);
CREATE INDEX idx_deps_on ON dependencies(depends_on);
```

### 8.3 Transactional Installation Flow

```
Installation flow:
1. Resolve dependencies → build installation order (topological sort)
2. Check for conflicts (file conflicts, package conflicts)
3. BEGIN TRANSACTION
4. For each package:
   a. Extract to temporary directory
   b. Record file manifest in database
   c. Copy files to system directories
   d. Record operation in transactions table
5. Execute post_install scripts
6. COMMIT TRANSACTION

Failure rollback:
1. Delete already-copied files
2. Restore overwritten files (from backup)
3. Remove records from database
4. ROLLBACK TRANSACTION
```

### 8.4 Dependency Resolver

Initial implementation uses a simple topological sort algorithm:

```
Input: List of packages to install
Process:
  1. Recursively expand all runtime dependencies
  2. Build a directed dependency graph
  3. Detect circular dependencies (error out)
  4. Topological sort to determine installation order
  5. Verify version constraints are satisfied
Output: Ordered installation list
```

Version comparison follows semantic versioning (semver) conventions.

---

## 9. Build Tool (wright-build)

### 9.1 Command-Line Interface

```
wright-build <port_path>              # Build the specified port
wright-build <port_path> --stage <s>  # Execute only up to a specific stage
wright-build --clean <port_path>      # Clean build directory
wright-build --lint <port_path>       # Validate package.toml syntax
wright-build --rebuild <port_path>    # Force rebuild
```

### 9.2 Complete Build Flow

```
1.  Parse package.toml
2.  Validate all required fields
3.  Check that build dependencies are installed
4.  Create build working directory: /tmp/wright-build/{name}-{version}/
    ├── src/        (SRC_DIR)
    ├── pkg/        (PKG_DIR)
    └── log/        (build logs)
5.  fetch: Download source to cache, symlink to src/
6.  verify: Validate SHA-256 checksums
7.  extract: Unpack source archive into src/
8.  For each user-defined stage (prepare → ... → post_package):
    a. Load executor definition
    b. Perform variable substitution on script content
    c. Generate bwrap command
    d. Execute and capture output, write to log/
    e. Check exit code
9.  Generate file manifest from pkg/
10. Strip binaries (if options.strip = true)
11. Generate package metadata file .PKGINFO
12. Archive: tar -c -I 'zstd -19' -f {name}-{version}-{release}-{arch}.wright.tar.zst -C pkg/ .
13. Compute package SHA-256
14. Move to output directory
```

### 9.3 Binary Package Internal Structure

```
{name}-{version}-{release}-{arch}.wright.tar.zst
├── .PKGINFO        # Package metadata (TOML format)
├── .FILELIST       # File manifest (one path per line)
├── .INSTALL        # Install scripts (if any)
└── usr/            # Actual installed files
    ├── bin/
    ├── lib/
    ├── share/
    └── ...
```

`.PKGINFO` format:

```toml
[package]
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
arch = "x86_64"
license = "BSD-2-Clause"
install_size = 2847563
build_date = "2025-01-15T10:30:00Z"
packager = "wright-build 0.1.0"

[dependencies]
runtime = ["openssl >= 3.0", "pcre2 >= 10.42", "zlib >= 1.2"]

[backup]
files = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]
```

---

## 10. Global Configuration

### 10.1 Main Configuration File

```toml
# /etc/wright/wright.toml

[general]
arch = "x86_64"                     # System architecture
hold_dir = "/var/hold"              # Hold tree path
cache_dir = "/var/lib/wright/cache"   # Cache directory
db_path = "/var/lib/wright/db/packages.db"
log_dir = "/var/log/wright"

[build]
build_dir = "/tmp/wright-build"       # Build working directory (tmpfs recommended)
default_sandbox = "strict"          # Default sandbox level
jobs = 0                            # Parallel compilation count, 0 = auto-detect nproc
cflags = "-O2 -pipe -march=x86-64"
cxxflags = "${cflags}"
strip = true
ccache = false

[network]
download_timeout = 300              # Download timeout (seconds)
retry_count = 3                     # Download retry attempts

[repos]
# Remote binary package repositories (later phase)
# [[repos.remote]]
# name = "official"
# url = "https://repo.example.com/packages"
# priority = 100
```

---

## 11. Rust Project Structure

```
wright/
├── Cargo.toml
├── Cargo.lock
├── src/
│   ├── bin/
│   │   ├── wright.rs                 # Package manager entry point
│   │   ├── wright_build.rs           # Build tool entry point
│   │   └── wright_repo.rs            # Repository tool entry point
│   ├── lib.rs                      # Library root
│   ├── config.rs                   # Global configuration loading
│   ├── package/
│   │   ├── mod.rs
│   │   ├── manifest.rs             # package.toml parsing (serde deserialization)
│   │   ├── version.rs              # Version comparison (semver)
│   │   └── archive.rs              # Binary package packing/unpacking
│   ├── resolver/
│   │   ├── mod.rs
│   │   ├── graph.rs                # Dependency graph construction
│   │   └── topo.rs                 # Topological sort
│   ├── database/
│   │   ├── mod.rs
│   │   └── schema.rs               # SQLite table schema and operations
│   ├── transaction/
│   │   ├── mod.rs
│   │   └── rollback.rs             # Transaction management and rollback
│   ├── builder/
│   │   ├── mod.rs
│   │   ├── lifecycle.rs            # Lifecycle pipeline scheduling
│   │   ├── executor.rs             # Executor loading and invocation
│   │   └── variables.rs            # Variable substitution engine
│   ├── sandbox/
│   │   ├── mod.rs
│   │   └── bwrap.rs                # bubblewrap command generation and invocation
│   ├── repo/
│   │   ├── mod.rs
│   │   ├── index.rs                # Repository index generation and parsing
│   │   ├── sync.rs                 # Remote repository synchronization
│   │   └── source.rs               # Source resolution (priority-based)
│   └── util/
│       ├── mod.rs
│       ├── download.rs             # HTTP downloads
│       ├── checksum.rs             # SHA-256 verification
│       └── compress.rs             # tar.zst compression/decompression
└── tests/
    ├── integration/
    │   ├── build_test.rs           # Build flow integration tests
    │   ├── install_test.rs         # Install/uninstall integration tests
    │   └── sandbox_test.rs         # Sandbox isolation tests
    └── fixtures/
        ├── simple-pkg/             # Simple test package
        │   └── package.toml
        └── dep-chain/              # Dependency chain test packages
            ├── liba/
            └── libb/
```

---

## 12. Development Roadmap

### Phase 1: Minimum Viable Product (MVP)

Goal: Ability to build, install, and uninstall packages.

Tasks:
- [ ] package.toml parser (manifest.rs)
- [ ] Shell executor
- [ ] Basic lifecycle pipeline (prepare → build → package)
- [ ] Unsandboxed builds (sandbox = none)
- [ ] tar.zst packaging
- [ ] SQLite database + file manifest tracking
- [ ] `wright install/remove/list/files/owner` commands
- [ ] Basic dependency checking (no automatic installation)

### Phase 2: Sandbox + Dependencies

Goal: Secure isolated builds, automatic dependency resolution.

Tasks:
- [ ] bubblewrap sandbox integration
- [ ] strict / relaxed sandbox levels
- [ ] Topological sort dependency resolution
- [ ] Automatic dependency installation
- [ ] Version constraint checking
- [ ] `wright upgrade` command
- [ ] Transactional install + rollback

### Phase 3: Multi-Executor + Hooks

Goal: Complete extensible build system.

Tasks:
- [ ] Executor plugin system
- [ ] Python / Lua executors
- [ ] pre/post hook support
- [ ] Custom lifecycle pipelines
- [ ] Build log auditing
- [ ] ccache integration
- [ ] `wright-build --lint` package description validation

### Phase 4: Repository + Distribution

Goal: Remote repository support and self-hosting capability.

Tasks:
- [ ] Repository index format (`index.toml`) generation
- [ ] `wright-repo generate` tool for creating repository from built packages
- [ ] `wright sync` remote repository synchronization with caching
- [ ] Priority-based source resolution (local > remote)
- [ ] Remote package download with SHA-256 verification
- [ ] GPG signature verification for repository index
- [ ] Mirror support with fallback
- [ ] Offline/air-gapped installation support
- [ ] `repos.toml` configuration parsing
- [ ] Delta upgrades (optional, low priority)

### Phase 5: Distribution Bootstrap

Goal: A self-hosting base system that can rebuild itself.

Tasks:
- [ ] Core tier holds: musl libc, Linux kernel headers, busybox, GCC, binutils, make, bash
- [ ] Base tier holds: runit, wright (self-hosting), openssl/libressl, curl, git, zstd
- [ ] musl compatibility patch collection for common packages
- [ ] runit service directory templates and conventions
- [ ] Flatpak integration documentation for desktop application delivery
- [ ] Bootstrap script: from LFS base to a wright-managed system
- [ ] ISO/image generation tooling (optional)

---

## 13. Repository System

### 13.1 Overview

The wright repository system supports both local hold trees (source-based) and remote binary repositories. The design prioritizes simplicity, verifiability, and offline capability.

### 13.2 Repository Index Format

Each binary repository contains a single index file that describes all available packages:

```
repo/
├── index.toml              # Repository index (metadata for all packages)
├── index.toml.sig          # GPG detached signature of index.toml
└── packages/
    ├── nginx-1.25.3-1-x86_64.wright.tar.zst
    ├── openssl-3.2.1-1-x86_64.wright.tar.zst
    └── ...
```

#### index.toml Format

```toml
[repository]
name = "official"
description = "Wright Official Repository"
arch = "x86_64"
generated_at = "2025-06-15T10:30:00Z"
generator = "wright-build 0.1.0"

# Each package is an entry in the packages array
[[packages]]
name = "nginx"
version = "1.25.3"
release = 1
description = "High performance HTTP and reverse proxy server"
arch = "x86_64"
license = "BSD-2-Clause"
install_size = 2847563
download_size = 892416
filename = "nginx-1.25.3-1-x86_64.wright.tar.zst"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"
depends = ["openssl >= 3.0", "pcre2 >= 10.42", "zlib >= 1.2"]
conflicts = ["apache"]
provides = ["http-server"]
build_date = "2025-06-14T08:00:00Z"
group = "extra"

[[packages]]
name = "openssl"
version = "3.2.1"
release = 1
# ... same fields as above
```

### 13.3 Repository Source Configuration

```toml
# /etc/wright/repos.toml

# Local hold tree (always available)
[[source]]
name = "hold"
type = "hold"
path = "/var/hold"
priority = 100              # Higher priority = preferred

# Official binary repository
[[source]]
name = "official"
type = "remote"
url = "https://repo.example.com/x86_64"
priority = 200
gpg_key = "/etc/wright/keys/official.gpg"     # Optional: GPG public key for verification
enabled = true

# Mirror
[[source]]
name = "mirror-us"
type = "remote"
url = "https://us.mirror.example.com/x86_64"
priority = 150
gpg_key = "/etc/wright/keys/official.gpg"
enabled = true

# Local binary cache (for self-built packages)
[[source]]
name = "local"
type = "local"
path = "/var/lib/wright/cache/packages"
priority = 300              # Highest priority — prefer locally built packages
```

Resolution order: When multiple sources provide the same package, the source with the highest priority wins. Within the same priority, the newest version wins.

### 13.4 Repository Synchronization

`wright sync` performs the following:

```
1. For each enabled remote source:
   a. Download index.toml (with If-Modified-Since for efficiency)
   b. If GPG key is configured, download index.toml.sig and verify signature
   c. Parse and validate index.toml
   d. Store locally in /var/lib/wright/cache/repos/{name}/index.toml
2. Merge all source indexes into a unified view
3. Report: new packages, available upgrades, removed packages
```

### 13.5 Repository Generation

The `wright-repo` tool (or `wright-build --repo`) generates a repository from a directory of built packages:

```
wright-repo generate /path/to/packages/ --output /path/to/repo/
```

Process:
1. Scan directory for `*.wright.tar.zst` files
2. Extract `.PKGINFO` from each package
3. Compute SHA-256 and file sizes
4. Generate `index.toml`
5. Optionally sign with GPG: `gpg --detach-sign index.toml`

### 13.6 Package Download and Verification

```
1. Resolve package to a source (priority-based)
2. Check local cache first (/var/lib/wright/cache/packages/)
3. If not cached, download from remote URL
4. Verify SHA-256 against index.toml entry
5. If verification fails, delete and retry (up to retry_count)
6. Cache the verified package locally
7. Pass to the install subsystem
```

### 13.7 Offline and Air-Gapped Support

The repository system must support fully offline operation:

- `wright install <path/to/file.wright.tar.zst>` installs directly from a local file
- The hold tree (`/var/hold/`) works entirely offline once source tarballs are cached
- A USB-based repository is just a directory with `index.toml` + packages — mount and point a `[[source]]` at it
- `wright-build` caches all downloaded sources in `/var/lib/wright/cache/sources/` for future offline rebuilds

### 13.8 Repository Hosting

A wright repository is a **static file directory**. No server-side logic is needed. Any HTTP server, rsync target, or even a mounted USB drive can serve as a repository. This is intentional — it maximizes hosting flexibility and minimizes infrastructure requirements.

Recommended hosting options:
- Static file hosting (nginx, caddy, S3, GitHub Releases)
- rsync for mirror synchronization
- Local filesystem or mounted media for offline use

---

## 14. Design Constraints and Considerations

### 14.1 Security Constraints

- Build scripts **must never** run as root outside the sandbox
- `install_scripts` (post_install, etc.) are the **only** scripts that run as root on the real system; the user must be explicitly warned during installation
- Executor `command` must be an absolute path pointing to an existing executable file
- Source SHA-256 verification failure **must** abort the build; skipping is not allowed
- Network access is **forbidden** in strict sandbox mode

### 14.2 Compatibility Constraints

- Target system: x86_64 Linux (based on LFS 11.x+)
- C library: **musl libc** (not glibc — see Section 1.5 for rationale and compatibility policy)
- Init system: **runit** (no systemd dependency anywhere in the toolchain)
- Minimum kernel version: 5.10+ (namespace support)
- Requires bubblewrap >= 0.5.0
- Filesystem: Must support overlay or bind mount capable filesystems
- All packages must build and run against musl; glibc-only software is excluded from the native repository
- No hard dependency on systemd, dbus, polkit, or logind in core/base tier packages

### 14.3 Performance Targets

- Package install/uninstall operations should complete in seconds (excluding download time)
- Database queries should respond in < 100ms
- Dependency resolution (up to 100 packages) should complete in < 1s
- Package extraction speed is limited by disk I/O; zstd decompression overhead is negligible

### 14.4 Error Handling Principles

- All user-facing errors must have clear messages and suggested remediation steps
- Build failures should preserve the build directory and logs for debugging
- Database operations must use transactions; any failure must roll back to a consistent state
- Network errors should automatically retry (up to 3 times) with clear timeout messages

---

## 15. Testing Strategy

### 15.1 Unit Tests

- TOML parsing: Various valid/invalid package.toml inputs
- Version comparison: semver edge cases
- Dependency graph: Cycle detection, topological sort correctness
- Variable substitution: All variables expand correctly

### 15.2 Integration Tests

- End-to-end build of a simple C program package
- Install → verify files exist → uninstall → verify files removed
- Dependency chain installation: A depends on B depends on C
- Conflict detection: file conflicts, package conflicts
- Rollback: Verify system is clean after a mid-install failure
- Sandbox: Verify build scripts cannot access files or network outside the sandbox

### 15.3 Simple Test Package

```toml
# tests/fixtures/hello/package.toml
[package]
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test package"
license = "MIT"
arch = "x86_64"

[dependencies]
runtime = []
build = ["gcc"]

[sources]
urls = []
sha256 = []

[lifecycle.prepare]
executor = "shell"
sandbox = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.build]
executor = "shell"
sandbox = "none"
script = """
gcc -o hello hello.c
"""

[lifecycle.package]
executor = "shell"
sandbox = "none"
script = """
install -Dm755 hello ${PKG_DIR}/usr/bin/hello
"""
```
