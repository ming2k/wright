# How to Write a Plan

A **plan** is a directory containing a `plan.toml` file that describes how to fetch, build, and produce a **part** from a piece of software.

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

If a plan needs a separate bootstrap/MVP override, place it in a sibling `mvp.toml`. The base file remains `plan.toml`; do not rename it to `main.toml` or `base.toml`.

## Version, Release, and Epoch

Three fields in `[plan]` metadata control how Wright orders and names archives:

| Field | Purpose | Rule |
|-------|---------|------|
| `version` | Upstream release identifier | Copy the upstream version string exactly. Omit only for rolling or VCS builds with no static version. |
| `release` | Build revision within the same `version` | Bump when the plan changes (patches, build flags, dependencies) while upstream stays the same. **Reset to `1` on every `version` bump.** |
| `epoch` | Forced ordering override | Bump **only** when upstream changes its versioning scheme so the new `version` would sort lower than the old one (e.g. `2024.1` → `1.0.0`). Never decreases. Omit (default `0`) for normal releases. |

Example lifecycle:

```
Upstream releases 1.0.0  → version = "1.0.0", release = 1
Add a patch              → version = "1.0.0", release = 2
Upstream releases 1.1.0  → version = "1.1.0", release = 1 (reset)
Upstream renames to 2.0  → version = "2.0",   release = 1, epoch = 1
```

## Define Dependencies

### Build-Time Dependencies

Use `build_deps` for tools and headers needed during compilation. Use `link_deps` for shared libraries your binary actually links against at build time:

```toml
build_deps = ["pkg-config"]
link_deps = ["openssl", "zlib"]
```

Put version constraints after the output reference:

```toml
link_deps = ["pcre2 >= 10.42"]
```

`wright lint` validates that each referenced local plan exists and that the referenced output is declared by that plan.

For a multi-output plan, use the concrete output name when you only need one:

```toml
# llvm produces multiple outputs: llvm, llvm-libs, clang, lld
build_deps = ["llvm:clang", "llvm:lld"]
```

Writing `llvm-libs:default` would mean a **separate plan** named `llvm-libs`, not the `llvm-libs` output of the `llvm` plan. Always use `plan:output` for specific outputs.

### Runtime Dependencies

Runtime dependencies are declared per-output, because they describe what a specific installed part needs at run time. Use `runtime_deps` directly inside each `[[output]]` entry:

```toml
[[output]]
name = "nginx"
runtime_deps = ["openssl", "zlib"]

[[output]]
name = "nginx-minimal"
runtime_deps = ["openssl"]
include = ["/usr/sbin/nginx", "/etc/nginx/nginx.conf"]

[[output]]
name = "nginx-modules"
runtime_deps = ["openssl", "zlib", "pcre2"]
include = ["/usr/lib/nginx/modules/**"]
```

Rules:

- `runtime_deps` is **output-level only** — there is no plan-level fallback.
- Each output declares exactly what it needs. Outputs are independent; one output's deps do not affect another.
- `build_deps` and `link_deps` are **plan-level only** — they drive build planning and have no meaning inside `[[output]]`.

## Fetch Sources

Sources use TOML's array-of-tables syntax with a mandatory `type` field.

### Download a Remote Archive

```toml
[[sources]]
type = "http"
url = "https://nginx.org/download/nginx-${VERSION}.tar.gz"
sha256 = "a51897b1e37e9e73e70d28b9b12c9a31779116c15a1115e3f3dd65291e26bd83"
```

### Clone a Git Repository

```toml
[[sources]]
type = "git"
url = "https://github.com/example/repo.git"
ref = "v1.2.3"
depth = 1
```

You can use variable substitution in `ref` (e.g. `ref = "v${VERSION}"`) to tie the checkout tag to the plan version.

### Include Local Files

```toml
[[sources]]
type = "local"
path = "patches/fix-headers.patch"
```

Local paths are relative to the plan directory and must not escape it.

## Apply Patches

Patches are **not** auto-applied. Include them as `type = "local"` entries and apply them manually in a lifecycle stage. This gives full control over strip level, ordering, and conditions:

```toml
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

## Configure Build Options

Use `[options]` to set per-plan build behavior:

```toml
[options]
static = false
debug = false
ccache = true
env = { CFLAGS = "-O2 -pipe" }
```

Per-plan values override global (`wright.toml`) settings. `memory_limit` and `cpu_time_limit` are enforced via `setrlimit()` before `exec` and inherited by child processes. The wall-clock `timeout` is enforced by the parent process — it catches builds stuck on I/O or deadlocks where CPU time does not advance.

**Practical guidance:** `timeout` is the most important safety net. `memory_limit` limits virtual address space (`RLIMIT_AS`), not physical RSS — set it generously (2-3x expected usage), as programs like rustc, JVM, and Go reserve large virtual mappings they never touch.

## Write Lifecycle Scripts

Each lifecycle stage is a TOML table under `lifecycle`:

```toml
[lifecycle.compile]
executor = "shell"
isolation = "strict"
script = """
make -j$(nproc)
"""
```

The default pipeline order is: `fetch`, `verify`, `extract`, `prepare`, `configure`, `compile`, `check`, `staging`.

Override this order if your build needs a different pipeline:

```toml
[lifecycle_order]
stages = ["fetch", "verify", "extract", "configure", "compile", "staging"]
```

### Pre/Post Hooks

Any stage can have a pre- or post-hook:

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

Execution order: `pre_<stage>` → `<stage>` → `post_<stage>`.

## Choose an Isolation Level

| Build tool / scenario | Level | Reason |
|-----------------------|-------|--------|
| C/C++ — autotools, CMake, meson | `strict` | No network needed at build time; default is correct |
| Rust — `cargo build` | `relaxed` | Cargo fetches crates from crates.io during compilation unless vendored |
| Go — `go build` / `go mod download` | `relaxed` | Go modules download from proxy.golang.org during build unless vendored |
| Node.js — `npm install` / `yarn` | `relaxed` | Package manager downloads from npm registry during install |
| Python — `pip install` / `python setup.py` | `relaxed` | pip fetches from PyPI during install |
| Stage needs host IPC | `relaxed` | IPC namespace is not isolated, so System V / POSIX queues remain accessible |
| Stage needs full host access | `none` | No namespace isolation at all — use only when unavoidable |

The recommended pattern for network-fetching build tools (Cargo, Go, npm) is to pre-vendor dependencies and build fully offline under `strict`:

- **Cargo**: include a `vendor/` directory and set `CARGO_NET_OFFLINE=true` plus a `.cargo/config.toml` pointing at the vendor dir.
- **Go**: run `go mod vendor` and pass `-mod=vendor` at build time.
- **npm**: include `node_modules/` in the source archive or use `npm pack`/offline mirror.

When vendoring is not practical (e.g. bootstrapping the toolchain itself), use `relaxed` so the build can reach the network while still keeping a private filesystem and process namespace.

## Declare Install Hooks

`[output.hooks]` contains transaction-time scripts that run on the live system, not in the isolated build lifecycle:

```toml
[[output]]

[output.hooks]
pre_install = "echo 'Preparing installation...'"
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"
```

Hooks run on the live system, serially, blocking the install. Keep them fast. For operations that are inherently slow and single-threaded (e.g. `fmtutil-sys --all`, `texhash`, font cache generation), prefer running only the subset needed at install time and let the user invoke the full regeneration manually afterward.

## Choose an Output Mode

Wright has implicit and explicit output modes.

### Mode 1: No Output Section (Default)

Omit `[[output]]`. The plan produces exactly one part named after the top-level `name` field. Everything installed into `${STAGING_DIR}` during staging becomes that part.

```toml
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = """
install -Dm755 hello ${STAGING_DIR}/usr/bin/hello
"""
```

Result: one part named `hello` containing `/usr/bin/hello`.

### Mode 2: Explicit Single Output (`[[output]]`)

Use `[[output]]` when you need hooks, backup files, runtime dependencies, part relations, discard rules, or explicit coverage for a single part. Omit `name` or set it to `""` to use the plan name.

```toml
name = "nginx"
# ...

[[output]]
conflicts = ["apache"]
provides = ["http-server"]
backup = ["/etc/nginx/nginx.conf"]

[output.hooks]
post_install = "useradd -r nginx 2>/dev/null || true"
```

### Mode 3: Split Outputs (`[[output]]`)

Use `[[output]]` to route staging files through explicit output rules. This can produce one part or many parts. Omit `name` or set it to `""` on one output to use the plan name. Other fields depend on whether the entry is a catch-all or not.

```toml
name = "gcc"
version = "14.2.0"
release = 1
description = "The GNU Compiler Collection"
license = "GPL-3.0-or-later"
arch = "x86_64"

[[output]]
name = "gcc"
# catch-all — keeps everything not claimed by earlier outputs

[[output]]
name = "libstdc++"
description = "GNU C++ standard library"
include = ["/usr/lib/libstdc*"]
runtime_deps = ["libgcc"]
```

**Coverage rules:**

- Every staged file must be claimed by one `[[output]]`, matched by `[[discard]]`, or claimed by the optional catch-all
- Plans with unclaimed staged files fail during output slicing
- At most one catch-all is allowed
- `description` is **not** required for catch-all outputs

Use `[[discard]]` for files that are intentionally not packaged. It is always
an array-of-tables, even for one rule, so each ignored file group can carry its
own `reason`.

#### Implicit Slicing

Wright uses a **Single-Source Staging, Multi-Target Slicing** architecture. All files should be installed to `${STAGING_DIR}` during the `staging` lifecycle phase. On the host, that directory is `build_dir/<name>-<version>/staging`; inside isolation it is mounted at `/output`. After `staging` is complete, an implicit slicing engine processes the files based on the `[[output]]` definitions.

**Output processing order:**

1. Non-catch-all outputs (those with explicit `include` patterns) are processed **in their declared order**.
2. For each non-catch-all output, files matching its `include` patterns (and not matching its `exclude` patterns) are **hard-linked** from `${STAGING_DIR}` into the respective output directory.
3. A file is claimed by the **first** output whose `include` matches it. Later outputs never see it.
4. Remaining files matching `[[discard]]` are ignored.
5. The optional catch-all output (the one with no `include`) packages whatever remains after earlier outputs and discard rules have handled their files.
6. Any file still unclaimed fails slicing.

**Critical: `include` patterns must be specific.** Using `include = ["/**"]` for a non-catch-all output will greedily capture **all** files, leaving nothing for later outputs and nothing for the catch-all. Each non-catch-all output should only match the files that belong to it.

#### Part Relations

Relations are **per-output**, not per-plan. In multi-output mode, each sub-part declares its own relations independently.

- **`replaces`** — Automatic migration. When installing this part, Wright silently removes any installed part whose name appears in this list. Use for part renames and merges (e.g. `nginx-mainline` replaces `nginx`).

- **`conflicts`** — Mutual exclusion. Wright refuses to install this part while a conflicting part is present (or vice versa). Use when two parts provide overlapping functionality and cannot coexist (e.g. `nginx` and `apache` both binding port 80). Conflicts are **bidirectional**.

- **`provides`** — **Deprecated.** Still parsed for plan-source compatibility but no longer recognized at runtime. Virtual aliasing has been retired in favour of the advisory runtime model: depend on a concrete `plan:output`, and use `replaces` to handle renames or splits. See [ADR-0016](../adr/0016-advisory-runtime-dependencies.md).

#### Backup Files

Files listed in `backup` are treated as **user-owned config files**:

- **On upgrade:** the new default is always written alongside as `<path>.wnew` (e.g. `/etc/nginx/nginx.conf.wnew`) and a warning is printed. The live file is left intact so user customisations are never lost.
- **On remove:** config files are **not deleted**, even when the part is removed.

## Handle Circular Dependencies

Some parts have genuine circular build-time dependencies. Wright resolves them automatically using a **two-pass build**:

1. **MVP pass** — builds the part without the cyclic dependency (functional but reduced).
2. **Full pass** — after the rest of the cycle is built, rebuilds the part with all dependencies.

Place a `mvp.toml` file next to `plan.toml` with MVP-specific dependencies so the graph becomes acyclic:

```text
foo/
├── plan.toml
└── mvp.toml
```

`mvp.toml`:

```toml
link_deps = ["freetype"] # omit harfbuzz in MVP
```

Wright's planning layer uses Tarjan's SCC algorithm to detect cycles. If it finds a cycle and a plan in that cycle has MVP overrides that remove at least one edge of the cycle, it automatically inserts the two-pass schedule.

The MVP phase can also be triggered **manually** without a cycle being present, using the `--mvp` flag:

```bash
wright build freetype --mvp
```

For complex parts, provide **dedicated MVP scripts** instead of embedding conditionals. Place the override lifecycle stages inside `mvp.toml`; they are used **only during the MVP pass**:

```toml
# mvp.toml
link_deps = ["cairo", "pango", "glib"]

[lifecycle.configure]
script = """
meson setup build \
 --prefix=/usr \
 -Dpixbuf=disabled
"""
```

Resolution order for the MVP pass:

1. If `[lifecycle.<stage>]` exists in `mvp.toml`, it is used.
2. Otherwise, it falls back to `[lifecycle.<stage>]` in `plan.toml`.

See [How to Handle Circular Dependencies](handle-circular-dependencies.md) for more details.

## Source Organization Style

### 1. Single Main Archive (Recommended)

For plans with one primary source code archive, **always** use `extract_to = "source"`. This is a mandatory defensive convention.

- **Why**: Wright does not normalize or strip the internal directory structure of archives. Some upstream tarballs have a single top-level folder, while others are "flat" and extract files directly into the current directory. Without `extract_to`, a flat tarball would pollute the root of `${WORKDIR}`.
- **Benefit**: Every plan has a single, predictable root for its main source tree.
- **Lifecycle scripts must descend further**: After extraction, your build scripts must navigate into the actual source directory inside `source/`. Most archives include a top-level folder, so the correct pattern is `cd source/<project>-${VERSION}`, **not** merely `cd source`.

```toml
[[sources]]
type = "http"
url = "https://example.com/bash-completion-${VERSION}.tar.gz"
sha256 = "..."
extract_to = "source"

[lifecycle.configure]
script = """
cd source/bash-completion-${VERSION}
./configure --prefix=/usr
"""
```

**Never** omit the inner directory in your `cd` command unless you have verified that the archive is genuinely flat.

To inspect a tarball's structure:

```bash
tar tzf foo-1.0.tar.gz | head -n 20
```

### 2. Patches and Single Files

Do **not** use `extract_to` for individual files like patches or configuration templates. Leave `extract_to` unset so files are placed directly in `${WORKDIR}`:

```toml
[[sources]]
type = "local"
path = "fix-build.patch"

[lifecycle.compile]
script = "cd source && patch -p1 < ${WORKDIR}/fix-build.patch"
```

### 3. Multi-Component Builds

For complex builds involving multiple archives, give each one a unique, descriptive directory name:

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

## Scripting Robustness

- **Explicit Navigation**: Always start your scripts with an explicit `cd` if your work is in a subdirectory.
- **Variable Usage**: Prefer `${WORKDIR}/filename` over relative paths for clarity.
- **Cleanup**: Don't worry about cleaning up `${WORKDIR}` or `${STAGING_DIR}`; Wright handles this automatically before each build.

## Examples

### Minimal Plan

```toml
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test part"
license = "MIT"
arch = "x86_64"

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
install -Dm755 hello ${STAGING_DIR}/usr/bin/hello
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

link_deps = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]

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
make DESTDIR=${STAGING_DIR} install
"""

[[output]]
name = "nginx"
conflicts = ["apache"]
provides = ["http-server"]
runtime_deps = ["openssl", "pcre2 >= 10.42", "zlib >= 1.2"]
backup = ["/etc/nginx/nginx.conf", "/etc/nginx/mime.types"]

[output.hooks]
pre_install = "echo 'Preparing nginx installation...'"
post_install = "useradd -r nginx 2>/dev/null || true"
post_upgrade = "systemctl reload nginx 2>/dev/null || true"
pre_remove = "systemctl stop nginx 2>/dev/null || true"

[[output]]
name = "nginx-doc"
description = "Nginx documentation files"
include = ["/usr/share/doc/**"]
```

### Multi-Output Examples

#### Library + Development Headers

Split runtime libraries from headers and static archives:

```toml
name = "libfoo"
version = "1.0.0"
release = 1
description = "Foo library"
license = "MIT"
arch = "x86_64"

[lifecycle.staging]
script = """
make DESTDIR=${STAGING_DIR} install
"""

[[output]]
name = "libfoo"
description = "Foo runtime libraries"
include = ["/usr/lib/libfoo.so*"]
runtime_deps = ["glibc"]

[[output]]
name = "libfoo-dev"
description = "Foo development files"
include = ["/usr/include/**", "/usr/lib/libfoo.a", "/usr/lib/pkgconfig/libfoo*"]
```

In this example, `libfoo` is the catch-all and packages files not claimed by `libfoo-dev`.

#### Meta-part with Sub-parts

Create a meta-part that depends on all sub-parts, useful for grouping:

```toml
name = "linux-firmware"
version = "20250101"
release = 1
description = "Linux firmware files"
license = "multiple"
arch = "x86_64"

[[output]]
name = "linux-firmware"
runtime_deps = ["linux-firmware-amd", "linux-firmware-intel", "linux-firmware-nvidia"]

[[output]]
name = "linux-firmware-amd"
description = "AMD GPU/CPU firmware"
include = ["/usr/lib/firmware/amdgpu/**", "/usr/lib/firmware/radeon/**"]

[[output]]
name = "linux-firmware-intel"
description = "Intel GPU/CPU firmware"
include = ["/usr/lib/firmware/i915/**", "/usr/lib/firmware/iwlwifi/**"]

[[output]]
name = "linux-firmware-nvidia"
description = "NVIDIA GPU firmware"
include = ["/usr/lib/firmware/nvidia/**"]
```

**Note:** The catch-all receives whatever the sub-parts do not claim. If you want a pure meta-part with no files, ensure the sub-parts claim all installed files; otherwise the catch-all will contain the leftovers.

#### Explicit Discard for Ignored Files

Keep specific files and explicitly ignore known unwanted files:

```toml
name = "llvm-tools"
version = "22.1.3"
release = 1
description = "Selected LLVM tools"
license = "Apache-2.0-with-LLVM-exception"
arch = "x86_64"

[[output]]
name = "llvm-opt"
description = "LLVM optimizer"
include = ["/usr/bin/opt", "/usr/bin/llvm-opt*"]

[[output]]
name = "llvm-dis"
description = "LLVM disassembler"
include = ["/usr/bin/llvm-dis", "/usr/bin/llvm-as"]

[[discard]]
include = [
    "/usr/share/doc/**",
    "/usr/share/man/**",
]
reason = "documentation and manual pages are intentionally not packaged"
```

If staging contains files like `clang`, `lld`, headers, or libraries, slicing fails until the plan assigns them to an output or explicitly discards them.

#### Override Architecture for Documentation

```toml
[[output]]
name = "mypart-doc"
description = "Documentation for mypart"
arch = "any"
include = ["/usr/share/doc/**"]
```

Sub-parts inherit `version`, `release`, `arch`, and `license` from the parent manifest unless overridden.
