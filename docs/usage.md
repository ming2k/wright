# Usage Guide

See [getting-started.md](getting-started.md) for prerequisites and installation.

## Directory Layout

Wright uses the following directory structure. All paths are configurable in `wright.toml` (see [configuration.md](configuration.md)).

### System layout (root)

```
/var/lib/wright/
├── plans/                  # Plan definitions (plan.toml per package)
│   ├── hello/
│   │   └── plan.toml
│   ├── nginx/
│   │   ├── plan.toml
│   │   └── patches/
│   └── ...
├── components/             # Built package archives (.wright.tar.zst)
├── cache/
│   └── sources/            # Downloaded source tarballs
└── db/
    └── packages.db         # Installed package database (SQLite)

/etc/wright/
├── wright.toml             # Global configuration
├── repos.toml              # Repository sources
├── executors/              # Executor definitions (*.toml)
│   └── shell.toml
└── assemblies/             # Assembly definitions (*.toml)
    └── core.toml

/var/log/wright/              # Operation logs

/tmp/wright-build/            # Build working directory (default)
```

### User layout (non-root, XDG)

```
~/.config/wright/wright.toml        # Per-user configuration
~/.cache/wright/sources/            # Source cache
~/.local/state/wright/              # Logs
```

### Build directory structure

During a build, wright creates the following layout under `build_dir` (default: `/tmp/wright-build`):

```
<build_dir>/<name>-<version>/
├── src/                    # Extracted source archives
│   └── <name>-<version>/   # Top-level source directory (= BUILD_DIR)
├── pkg/                    # Package output (install files here via PKG_DIR)
├── pkg-<split>/            # Split package output directories (one per split)
├── files/                  # Non-archive source files (patches, configs)
└── log/                    # Per-stage build logs
    ├── prepare.log
    ├── configure.log
    ├── compile.log
    ├── check.log
    └── package.log
```

Build directories are created fresh for each build. Use `wright build --clean <name>` to remove a previous build directory before rebuilding.

## Writing Plans

A plan is a directory containing a `plan.toml`. See [writing-plans.md](writing-plans.md) for the full reference.

Minimal example:

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
sandbox = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\n"); return 0; }
EOF
"""

[lifecycle.compile]
sandbox = "none"
script = "gcc -o hello hello.c"

[lifecycle.package]
sandbox = "none"
script = "install -Dm755 hello ${PKG_DIR}/usr/bin/hello"
```

For a plan that downloads upstream sources and uses link dependencies:

```toml
[plan]
name = "zlib"
version = "1.3.1"
release = 1
description = "Compression library"
license = "Zlib"
arch = "x86_64"

[dependencies]
build = ["gcc", "make"]
# No dependencies for zlib itself

[sources]
urls = ["https://zlib.net/zlib-${PKG_VERSION}.tar.gz"]
sha256 = ["9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"]

[lifecycle.configure]
script = "cd ${BUILD_DIR} && ./configure --prefix=/usr"

[lifecycle.compile]
script = "cd ${BUILD_DIR} && make -j${NPROC}"

[lifecycle.package]
script = "cd ${BUILD_DIR} && make DESTDIR=${PKG_DIR} install"
```

And a package linking against it:

```toml
[plan]
name = "curl"
# ...
[dependencies]
build = ["make", "perl"]
link = ["zlib", "openssl"]
```

## Validating and Updating

```
wright build --lint hello              # validate syntax only
wright build --update zlib             # download sources, fill in sha256
```

## Building

```
wright build hello
```

Plans are loaded from `plans_dir` (default: `/var/lib/wright/plans`). You can also pass a path directly.

Before building, Wright displays a **Construction Plan** showing what will be built and why:
- `[NEW]`: The target you requested, or a missing dependency that Wright found in the hold tree.
- `[LINK-REBUILD]`: Packages that depend on your target via `link` and must be rebuilt for ABI compatibility.
- `[REV-REBUILD]`: Transitive rebuilds requested via `--rebuild-dependents`.

### One-Stop Build and Install

The most efficient way to manage a package from source is using the `--install` (or `-i`) flag:

```
wright build -i curl
```

This command does the following:
1.  Analyzes `curl`'s dependencies.
2.  If any `build` or `link` dependencies are not installed, it searches for them in the hold tree (`plans_dir`).
3.  Recursively adds all missing plans to the construction plan.
4.  Starts parallel compilation.
5.  Immediately installs each package after it finishes building.

This ensures that `curl` (and all its requirements) are correctly installed on your system with a single command.

Lifecycle pipeline: fetch, verify, extract, prepare, configure, compile, check, package, post_package. Undefined stages are skipped. Each stage writes a log to `<build_dir>/<name>-<version>/log/<stage>.log` and also prints output to the terminal in real time.

### Clean Build Model

Wright uses a **clean build model**: each `wright build` invocation recreates `src/`, `pkg/`, and `log/` directories from scratch. This ensures build reproducibility and prevents stale artifacts from affecting the result, consistent with other source-based package managers (makepkg, rpmbuild, ebuild).

However, Wright does **not** re-download sources unnecessarily. The fetch stage caches downloaded tarballs in `cache/sources/` and verifies their SHA-256 checksums before reuse. Repeated builds of the same package skip the download entirely.

### Staged Builds

Use `--stage` to stop the pipeline after a specific stage, and `--only` to run a single stage in isolation:

```
wright build --stage configure hello      # stop after configure
wright build --stage compile hello        # stop after compile (skip check/package)
wright build --only compile hello         # run only the compile stage
wright build --only package hello         # run only the package stage
```

The build directory (`<build_dir>/<name>-<version>/`) is preserved after a staged build, so you can inspect intermediate results such as build logs, configured source trees, or compiled objects.

When `--only` is used, Wright preserves the existing `src/` directory (including configured/compiled source trees) and skips the fetch/verify/extract phases. This requires a previous build to exist — Wright will error if `src/` is not found.

**Typical workflow for iterating on a build:**

1. `wright build --stage configure hello` — run up to configure, then stop
2. Inspect logs, tweak the plan's compile script
3. `wright build --only compile hello` — rerun just the compile stage, reusing the configured source tree
4. Once everything works, `wright build hello` for a clean full build

**Typical workflow for debugging a build failure:**

1. `wright build --stage configure hello` — verify configure succeeds
2. Inspect `<build_dir>/hello-1.0.0/log/configure.log` for errors
3. Fix the plan, then `wright build hello` to run the full pipeline

For production use, always run a full clean build (no `--only`) to guarantee reproducible results.

### Build Flags

```
wright build hello                        # standard build
wright build --clean hello                # clean build directory first
wright build --force hello                # overwrite existing archive
wright build --stage configure hello      # stop after configure
wright build --only compile hello         # run only the compile stage
wright build -j4 hello zlib              # parallel builds
```

|               | Clean build dir | Overwrite archive |
|---------------|:---------------:|:-----------------:|
| `--clean`     | yes             | no                |
| `--force`     | no              | yes               |
| `--clean --force` | yes        | yes               |

### Assemblies

Group related plans for batch building:

```toml
# assemblies/core.toml
[assemblies.core]
description = "Core system packages"
plans = ["glibc", "gcc", "binutils", "make", "bash"]
```

```
wright build @core
```

## Installing

```
wright install hello-1.0.0-1-x86_64.wright.tar.zst
wright install --force pkg.wright.tar.zst         # reinstall
wright install --nodeps pkg.wright.tar.zst        # skip dependency checks
wright --root /mnt/target install pkg.wright.tar.zst  # alternate root
```

### Smart Transaction Handling

Wright automatically handles complex installation scenarios defined in the package manifest:

- **Replaces**: If a new package declares it `replaces` an existing one, Wright automatically uninstalls the old package (preserving configuration) before installing the new one. This is ideal for package renames.
- **Conflicts**: If a package `conflicts` with an already installed one, Wright will block the installation and error out to prevent system instability.
- **Upgrades**: `wright upgrade` rejects downgrades unless `--force` is used.

The installation process is fully atomic. If any step fails (including pre/post scripts), Wright rolls back changes to the previous consistent state.

### Version Handling

- `wright install` rejects if already installed
- `wright install --force` atomically replaces via upgrade path
- `wright upgrade` rejects downgrades unless `--force`

The upgrade process is atomic (backup, copy, remove old-only files, update DB, rollback on failure).

## Deploying into an LFS Chroot

Deploy Wright **after** the chroot has a working cross-toolchain, directory layout, and virtual filesystems.

### Static build

```bash
RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu
```

### Install and verify

```bash
install -m755 target/x86_64-unknown-linux-gnu/release/wright "$LFS/usr/bin/"
cp /etc/resolv.conf "$LFS/etc/resolv.conf"

# Inside chroot:
mkdir -p /var/lib/wright/{plans,components,cache,db}
mkdir -p /var/log/wright /etc/wright
wright build --lint <plan-name>
wright build --update <plan-name>   # tests network access
```

Wright uses `rustls` with bundled root certificates — no system CA bundle needed.
