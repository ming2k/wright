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

| Binary | Purpose |
|--------|---------|
| `wright` | Package manager (install, remove, query, verify) |
| `wright-build` | Build tool (parse plans, execute builds, create archives) |
| `wright-repo` | Repository tool (generate package index) |

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

| Directory | Purpose |
|-----------|---------|
| `/var/lib/wright/plans` | Plan directories (each containing a `package.toml`) |
| `/var/lib/wright/components` | Built `.wright.tar.zst` archives |
| `/var/lib/wright/cache` | Downloaded source tarballs |
| `/var/lib/wright/db` | Package database (`packages.db`) |
| `/var/log/wright` | Build stage logs |
| `/etc/wright` | System config, executor definitions, assemblies |
| `/tmp/wright-build` | Temporary build workspaces |

When running as a non-root user, Wright uses XDG directories instead:

| XDG path | Default |
|----------|---------|
| `$XDG_CACHE_HOME/wright` | `~/.cache/wright` |
| `$XDG_DATA_HOME/wright` | `~/.local/share/wright` |
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
urls = ["https://zlib.net/zlib-${version}.tar.gz"]
sha256 = ["9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"]

[lifecycle.configure]
script = """
cd zlib-${PKG_VERSION}
./configure --prefix=/usr
"""

[lifecycle.build]
script = """
cd zlib-${PKG_VERSION}
make -j${NPROC}
"""

[lifecycle.package]
script = """
cd zlib-${PKG_VERSION}
make DESTDIR=${PKG_DIR} install
"""
```

See [writing-plans.md](writing-plans.md) for the complete `package.toml` reference.

## 6. Validate a Plan

Check that a plan parses correctly without building anything:

```
wright-build --lint plans/hello
```

This validates all fields, checks the name regex, version format, and sha256 count.

## 7. Update Source Checksums

When adding or changing source URLs, use `--update` to automatically download the files and fill in the sha256 checksums:

```
wright-build --update plans/zlib
```

## 8. Build a Package

Build a single plan by pointing `wright-build` at the plan directory:

```
wright-build plans/hello
```

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

### Build options

```
wright-build plans/hello                # standard build
wright-build --clean plans/hello        # clean build directory first
wright-build --rebuild plans/hello      # clean + force rebuild
wright-build --force plans/hello        # overwrite existing archive
wright-build --stage configure plans/hello  # stop after configure
wright-build -j4 plans/hello plans/zlib     # build multiple plans, 4 parallel jobs
```

### Building by name

If plans are in a search path (`/var/lib/wright/plans`, `./plans`, etc.), you can build by package name instead of path:

```
wright-build hello
```

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
wright-build --lint plans/hello

# 5. Build
wright-build plans/hello

# 6. Install
wright install hello-1.0.0-1-x86_64.wright.tar.zst

# 7. Verify
wright query hello
wright files hello
hello              # run it

# 8. Clean up
wright remove hello
```

## Further Reading

- [Writing Plans](writing-plans.md) — complete `package.toml` reference
- [CLI Reference](cli-reference.md) — all commands and flags
- [Configuration](configuration.md) — `wright.toml` and `repos.toml`
- [Architecture](architecture.md) — codebase overview
