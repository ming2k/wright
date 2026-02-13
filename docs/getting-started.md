# Getting Started

## Prerequisites

- **Rust** (stable toolchain) — install via [rustup](https://rustup.rs/)
- **Linux** (x86_64) — wright targets LFS-based systems with kernel 5.10+
- **bubblewrap** (bwrap) >= 0.5.0 — required for sandboxed builds
- **bash** — default shell executor

Optional:
- **python3** — for the python executor
- **lua** — for the lua executor

## Building from Source

Clone the repository and build:

```
git clone <repo-url>
cd wright
cargo build --release
```

The three binaries are produced in `target/release/`:
- `wright` — package manager
- `wright-build` — build tool
- `wright-repo` — repository tool

Install them to your PATH (e.g., `/usr/local/bin/`):

```
install -m755 target/release/wright /usr/local/bin/
install -m755 target/release/wright-build /usr/local/bin/
install -m755 target/release/wright-repo /usr/local/bin/
```

## Configuration

wright works with no configuration file — all settings have sensible defaults. If you want to customize behavior, create `/etc/wright/wright.toml`. See [configuration.md](configuration.md) for details.

Default paths used when no config file exists:

| Setting | Default |
|---------|---------|
| Hold tree | `/var/hold` |
| Package database | `/var/lib/wright/db/packages.db` |
| Cache directory | `/var/lib/wright/cache` |
| Build directory | `/tmp/wright-build` |
| Log directory | `/var/log/wright` |

## Your First Package

### 1. Create a package description

Create a directory with a `package.toml`:

```
mkdir -p /var/hold/custom/hello
```

Write `/var/hold/custom/hello/package.toml`:

```toml
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

### 2. Build the package

```
wright-build /var/hold/custom/hello
```

This runs the lifecycle pipeline (prepare, build, package) and produces a `.wright.tar.zst` archive in the current directory.

### 3. Install the package

```
wright install hello-1.0.0-1-x86_64.wright.tar.zst
```

### 4. Upgrade the package

If you rebuild the package with a higher version or release, you can upgrade it:

```
wright upgrade hello-1.0.1-1-x86_64.wright.tar.zst
```

### 5. Verify the installation

```
wright query hello
wright files hello
wright verify hello
```

### 6. Remove the package

```
wright remove hello
```

## Next Steps

- [CLI Reference](cli-reference.md) — all commands and flags
- [Package Format](package-format.md) — full package.toml specification
- [Configuration](configuration.md) — customize wright's behavior
