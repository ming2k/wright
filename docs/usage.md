# Usage Guide

This guide walks through the full workflow of setting up Wright on a fresh system: installing the Rust toolchain, compiling Wright, writing a plan, building a package, and installing it.

## 1. Install the Rust Toolchain

Wright is written in Rust and requires a stable toolchain. Install it with [rustup](https://rustup.rs/):

```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Follow the prompts to install the default stable toolchain. Then make sure `cargo` is on your PATH:

```
source ~/.cargo/env
```

Verify the installation:

```
rustc --version
cargo --version
```

## 2. Build Wright

Clone the repository and compile in release mode:

```
git clone <repo-url>
cd wright
cargo build --release
```

This produces three binaries in `target/release/`:

| Binary         | Purpose                                                   |
|----------------|-----------------------------------------------------------|
| `wright`       | Package manager (install, remove, query, verify)          |
| `wright-build` | Build tool (parse plans, execute builds, create archives) |
| `wright-repo`  | Repository tool (generate package index)                  |

Install them somewhere on your PATH:

```
install -m755 target/release/wright /usr/local/bin/
install -m755 target/release/wright-build /usr/local/bin/
install -m755 target/release/wright-repo /usr/local/bin/
```

## 3. Create the Directory Layout

Wright uses several directories. They are created automatically as needed, but you may want to set them up in advance:

```
mkdir -p /var/lib/wright/{plans,components,cache,db}
mkdir -p /var/log/wright
mkdir -p /etc/wright
```

| Directory                    | Purpose                                             |
|------------------------------|-----------------------------------------------------|
| `/var/lib/wright/plans`      | Plan directories (each containing a `package.toml`) |
| `/var/lib/wright/components` | Built `.wright.tar.zst` archives                    |
| `/var/lib/wright/cache`      | Downloaded source tarballs                          |
| `/var/lib/wright/db`         | Package database (`packages.db`)                    |
| `/var/log/wright`            | Build stage logs                                    |
| `/etc/wright`                | System config, executor definitions, assemblies     |
| `/tmp/wright-build`          | Temporary build workspaces                          |

When running as a non-root user, Wright uses XDG directories instead:

| XDG path                 | Default                 |
|--------------------------|-------------------------|
| `$XDG_CACHE_HOME/wright` | `~/.cache/wright`       |
| `$XDG_DATA_HOME/wright`  | `~/.local/share/wright` |
| `$XDG_STATE_HOME/wright` | `~/.local/state/wright` |

## 4. Optional: Create a Configuration File

Wright works out of the box with sensible defaults. To customize, create `/etc/wright/wright.toml` (or `~/.config/wright/wright.toml` for per-user config):

```toml
[general]
arch = "x86_64"
plans_dir = "/var/lib/wright/plans"
components_dir = "/var/lib/wright/components"

[build]
build_dir = "/tmp/wright-build"
default_sandbox = "strict"
jobs = 0                              # 0 = auto-detect CPU count
cflags = "-O2 -pipe -march=x86-64"
cxxflags = "-O2 -pipe -march=x86-64"

[network]
download_timeout = 300
retry_count = 3
```

Config file lookup order:
1. `./wright.toml` (current directory)
2. `$XDG_CONFIG_HOME/wright/wright.toml` (non-root only)
3. `/etc/wright/wright.toml`

## 5. Write a Plan

A plan is a directory containing a `package.toml`. Here's a minimal example that compiles a C program from scratch:

```
mkdir -p plans/hello
```

Create `plans/hello/package.toml`:

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
sandbox = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\n"); return 0; }
EOF
"""

[lifecycle.build]
sandbox = "none"
script = """
gcc -o hello hello.c
"""

[lifecycle.package]
sandbox = "none"
script = """
install -Dm755 hello ${PKG_DIR}/usr/bin/hello
"""
```

For a plan that downloads upstream sources:

```toml
[package]
name = "zlib"
version = "1.3.1"
release = 1
description = "Compression library"
license = "Zlib"
arch = "x86_64"

[dependencies]
build = ["gcc", "make"]

[sources]
urls = ["https://zlib.net/zlib-${PKG_VERSION}.tar.gz"]
sha256 = ["9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"]

[lifecycle.configure]
script = """
cd ${BUILD_DIR}
./configure --prefix=/usr
"""

[lifecycle.build]
script = """
cd ${BUILD_DIR}
make -j${NPROC}
"""

[lifecycle.package]
script = """
cd ${BUILD_DIR}
make DESTDIR=${PKG_DIR} install
"""
```

See [writing-plans.md](writing-plans.md) for the complete `package.toml` reference.

## 6. Validate a Plan

Check that a plan parses correctly without building anything:

```
wright-build --lint hello
```

This validates all fields, checks the name regex, version format, and sha256 count.

## 7. Update Source Checksums

When adding or changing source URLs, use `--update` to automatically download the files and fill in the sha256 checksums:

```
wright-build --update zlib
```

## 8. Build a Package

Build a package by name:

```
wright-build hello
```

`wright-build` searches for plans in these directories (in order):
- `/var/lib/wright/plans`
- `./plans`
- `../plans`

You can also pass a path directly: `wright-build /path/to/hello` or `wright-build /path/to/hello/package.toml`.

This runs the full lifecycle pipeline:
1. **fetch** — download source archives (built-in)
2. **verify** — check sha256 checksums (built-in)
3. **extract** — unpack source archives (built-in)
4. **prepare** — apply patches, pre-build setup (user-defined)
5. **configure** — run configure scripts (user-defined)
6. **build** — compile (user-defined)
7. **check** — run tests (user-defined)
8. **package** — install into `${PKG_DIR}` (user-defined)
9. **post_package** — post-packaging steps (user-defined)

The output is a `.wright.tar.zst` archive placed in the components directory.

### Build logs

Each lifecycle stage writes a log file capturing the full stdout and stderr of the stage command. Logs are stored under the per-package build directory:

```
<build_dir>/<name>-<version>/log/<stage>.log
```

With the default `build_dir` of `/tmp/wright-build`, building `hello-1.0.0` produces:

```
/tmp/wright-build/hello-1.0.0/log/prepare.log
/tmp/wright-build/hello-1.0.0/log/build.log
/tmp/wright-build/hello-1.0.0/log/package.log
...
```

Output is printed to the terminal in real time and simultaneously captured to these files, so you can review a failed stage without scrolling through terminal history.

### Build options

```
wright-build hello                      # standard build
wright-build --clean hello              # clean build directory first
wright-build --rebuild hello            # clean + force rebuild
wright-build --force hello              # overwrite existing archive
wright-build --stage configure hello    # stop after configure
wright-build -j4 hello zlib            # build multiple plans, 4 parallel jobs
```

#### Flag comparison

|               | Clean build directory | Overwrite existing archive |
|---------------|:---------------------:|:--------------------------:|
| `--clean`     | yes                   | no                         |
| `--force`     | no                    | yes                        |
| `--rebuild`   | yes                   | yes                        |

- `--clean` — useful when the build directory has stale state from a previous attempt
- `--force` — useful when the archive already exists but you want to overwrite it without rebuilding from scratch
- `--rebuild` — equivalent to `--clean --force`, full clean rebuild

### Assemblies

Group related plans into assemblies for batch building. Create an assembly TOML file (e.g., `assemblies/core.toml`):

```toml
[assemblies.core]
description = "Core system packages"
plans = ["glibc", "gcc", "binutils", "make", "bash"]
```

Build an entire assembly with `@`:

```
wright-build @core
```

## 9. Install a Package

Install the built archive:

```
wright install hello-1.0.0-1-x86_64.wright.tar.zst
```

Install options:

```
wright install --force pkg.wright.tar.zst    # reinstall even if already installed
wright install --nodeps pkg.wright.tar.zst   # skip dependency checks
```

To install into an alternate root (useful for building target system images):

```
wright --root /mnt/target install hello-1.0.0-1-x86_64.wright.tar.zst
```

### Installing a different version of the same package

Wright does not support parallel installation of multiple versions. If a package with the same name is already installed:

- `wright install` — **rejects** with "package already installed"
- `wright install --force` — atomically replaces the old version via the upgrade path (see below)
- `wright upgrade` — the intended way to move to a newer version; rejects downgrades unless `--force` is used

The upgrade/force-install process is atomic — it is not a delete-then-install:

1. Backup existing files to a temporary directory
2. Copy new files into the system root
3. Remove files that only existed in the old version
4. Update the package database
5. If any step fails, rollback restores all backed-up files

Downgrading (`wright upgrade old-version.wright.tar.zst`) is rejected by default. Use `wright upgrade --force` to allow it.

## 10. Manage Installed Packages

### List installed packages

```
wright list
```

### Query package details

```
wright query hello
```

Output:
```
Name        : hello
Version     : 1.0.0
Release     : 1
Description : Hello World test package
Architecture: x86_64
License     : MIT
Install Size: 12345 bytes
Installed At: 2026-02-13T10:30:00Z
```

### List files owned by a package

```
wright files hello
```

### Find which package owns a file

```
wright owner /usr/bin/hello
```

### Search packages by keyword

```
wright search http
```

### Verify file integrity

```
wright verify hello    # verify one package
wright verify          # verify all installed packages
```

## 11. Upgrade a Package

Bump the version or release in the plan, rebuild, then upgrade:

```
wright upgrade hello-1.0.1-1-x86_64.wright.tar.zst
wright upgrade --force hello-1.0.1-1-x86_64.wright.tar.zst  # force even if not newer
```

## 12. Remove a Package

```
wright remove hello
```

## Complete Workflow Example

End-to-end: install Rust, build Wright, create a plan, build it, install it, verify it.

```bash
# 1. Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# 2. Build Wright
git clone <repo-url> && cd wright
cargo build --release
install -m755 target/release/wright /usr/local/bin/
install -m755 target/release/wright-build /usr/local/bin/

# 3. Create a plan
mkdir -p plans/hello
cat > plans/hello/package.toml << 'PLAN'
[package]
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World"
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

[lifecycle.build]
sandbox = "none"
script = "gcc -o hello hello.c"

[lifecycle.package]
sandbox = "none"
script = "install -Dm755 hello ${PKG_DIR}/usr/bin/hello"
PLAN

# 4. Validate
wright-build --lint hello

# 5. Build
wright-build hello

# 6. Install
wright install hello-1.0.0-1-x86_64.wright.tar.zst

# 7. Verify
wright query hello
wright files hello
hello              # run it

# 8. Clean up
wright remove hello
```

## Deploying into an LFS Chroot

Wright is designed to serve as the package manager for a Linux From Scratch (LFS) system. This section covers how to build Wright on the host, deploy it into the chroot, and ensure it works correctly inside the isolated environment.

### When to deploy

Deploy Wright into the chroot **after** the chroot environment is fully prepared — i.e., after you have:

1. Built the temporary cross-toolchain (binutils, gcc, glibc pass 1 & 2)
2. Created the basic LFS directory layout (`/usr`, `/etc`, `/var`, etc.)
3. Mounted the virtual kernel filesystems (`/dev`, `/proc`, `/sys`, `/run`)
4. Entered the chroot with `chroot "$LFS" /usr/bin/env -i ...`

At this point the chroot has a working shell and basic utilities, and is ready for Wright to take over package management for the remaining system packages.

### Build a static binary

Wright must be statically linked so it has no runtime dependency on the host's shared libraries (which may differ from what is available inside the chroot):

```bash
# On the host, in the wright source directory
RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu
```

This produces a fully static binary at `target/x86_64-unknown-linux-gnu/release/wright-build` (and `wright`, `wright-repo`).

### Install into the chroot

Copy the binaries and set up DNS resolution so that `wright-build --update` can download sources:

```bash
# Copy binaries into the chroot
install -m755 target/x86_64-unknown-linux-gnu/release/wright      "$LFS/usr/bin/"
install -m755 target/x86_64-unknown-linux-gnu/release/wright-build "$LFS/usr/bin/"
install -m755 target/x86_64-unknown-linux-gnu/release/wright-repo  "$LFS/usr/bin/"

# Enable DNS resolution inside the chroot
cp /etc/resolv.conf "$LFS/etc/resolv.conf"
```

### Create the wright directory layout inside the chroot

After entering the chroot:

```bash
mkdir -p /var/lib/wright/{plans,components,cache,db}
mkdir -p /var/log/wright
mkdir -p /etc/wright
```

### Verify

Inside the chroot, confirm everything works:

```bash
wright-build --version
wright-build --lint <plan-name>
wright-build --update <plan-name>   # tests network access
```

If `--update` fails with a network error, check that:

- `/etc/resolv.conf` exists and contains valid nameserver entries
- The virtual filesystems (`/proc`, `/sys`, `/dev`) are mounted
- The host network is accessible from the chroot (no network namespace isolation)

### Notes

- Wright uses `rustls` with bundled root certificates, so no system CA certificate bundle is needed inside the chroot for HTTPS downloads.
- Once glibc and the core toolchain are built as Wright packages inside the chroot, you can rebuild Wright itself as a dynamically linked Wright package for the final system.

## Further Reading

- [Writing Plans](writing-plans.md) — complete `package.toml` reference
- [CLI Reference](cli-reference.md) — all commands and flags
- [Configuration](configuration.md) — `wright.toml` and `repos.toml`
- [Architecture](architecture.md) — codebase overview
