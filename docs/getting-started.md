# Getting Started

## Prerequisites

- **Rust** stable toolchain — [rustup.rs](https://rustup.rs/)
- **Linux** x86_64, kernel 5.10+
- **bubblewrap** >= 0.5.0 — for sandboxed builds
- **bash**

## Build and Install

```
git clone <repo-url>
cd wright
cargo build --release
install -m755 target/release/wright /usr/local/bin/
```

## Configuration

Wright works with no config file. To customize, create `/etc/wright/wright.toml`. See [configuration.md](configuration.md).

## Your First Package

Create `plans/hello/plan.toml`:

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
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.build]
sandbox = "none"
script = "gcc -o hello hello.c"

[lifecycle.package]
sandbox = "none"
script = "install -Dm755 hello ${PKG_DIR}/usr/bin/hello"
```

Build, install, verify, remove:

```
wright build plans/hello
wright install hello-1.0.0-1-x86_64.wright.tar.zst
wright query hello
wright files hello
wright verify hello
wright remove hello
```

## Next Steps

- [Writing Plans](writing-plans.md) — complete plan.toml reference
- [CLI Reference](cli-reference.md) — all commands and flags
- [Usage Guide](usage.md) — build options, assemblies, LFS chroot deployment
- [Configuration](configuration.md) — customize wright's behavior
