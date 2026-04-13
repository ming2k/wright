# Getting Started

## Prerequisites

- Rust stable
- Linux x86_64
- `bubblewrap`
- `bash`

## Build

```bash
git clone <repo-url>
cd wright
cargo build --release
install -m755 target/release/wright /usr/local/bin/
```

## Mental Model

- `wright build` turns plans into local part archives.
- `wright build` registers those archives in a local inventory DB.
- `wright` installs and upgrades the live system from those local archives.
- `wright apply` is the high-level source-first combo workflow: resolve the
 build graph, add missing or outdated upstream dependency plans, build each
 wave, and install or upgrade each wave before continuing.

## First Part

Create `plans/hello/plan.toml`:

```toml
name = "hello"
version = "1.0.0"
release = 1
description = "Hello World test part"
license = "MIT"
arch = "x86_64"

[dependencies]
build = ["gcc"]

[lifecycle.prepare]
dockyard = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.compile]
dockyard = "none"
script = "gcc -o hello hello.c"

[lifecycle.staging]
dockyard = "none"
script = "install -Dm755 hello ${PART_DIR}/usr/bin/hello"
```

Build and install it:

```bash
wright build plans/hello
wright install hello
```

Or let Wright drive the whole source-first install/upgrade flow:

```bash
wright apply plans/hello
```

## Verify and Remove

```bash
wright query hello
wright files hello
wright verify hello
wright remove hello
```

## Next

- [Usage Guide](usage.md)
- [CLI Reference](cli-reference.md)
- [Configuration](configuration.md)
- [Writing Plans](writing-plans.md)
- [Writing Assemblies](writing-assemblies.md)
