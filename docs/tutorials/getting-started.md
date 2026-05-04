# Getting Started

## Prerequisites

- Rust stable
- Linux x86_64
- `bubblewrap`
- `bash`
- a C compiler for the sample plan

## Build

```bash
git clone <repo-url>
cd wright
cargo build --release
install -m755 target/release/wright /usr/local/bin/
```

## Mental Model

- `wright build` turns plans into staging directories.
- `wright package` turns build staging directories into local part archives.
- `wright` installs and upgrades the live system from archives in `parts_dir`.
- `wright apply` is the high-level source-first combo workflow: resolve the
 build graph, add missing or outdated dependency plans, build each
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

[lifecycle.prepare]
isolation = "none"
script = """
cat > hello.c << 'EOF'
#include <stdio.h>
int main() { printf("Hello, wright!\\n"); return 0; }
EOF
"""

[lifecycle.compile]
isolation = "none"
script = "gcc -o hello hello.c"

[lifecycle.staging]
isolation = "none"
script = "install -Dm755 hello ${STAGING_DIR}/usr/bin/hello"
```

You now have one plan directory, and the lifecycle scripts can use `${NAME}` /
`${VERSION}` for plan metadata.

Build and install it:

```bash
wright build plans/hello
wright package plans/hello
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

- [Usage Guide](../tutorials/first-steps.md)
- [CLI Reference](../reference/cli-reference.md)
- [Configuration](../reference/configuration.md)
- [How to Write a Plan](../how-to/write-a-plan.md)
