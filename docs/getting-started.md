# Getting Started

## Prerequisites

- **Rust** stable toolchain — [rustup.rs](https://rustup.rs/)
- **Linux** x86_64, kernel 5.10+
- **bubblewrap** >= 0.5.0 — for isolated dockyard builds
- **bash**

## Build and Install

```
git clone <repo-url>
cd wright
cargo build --release
install -m755 target/release/wright  /usr/local/bin/
install -m755 target/release/wbuild  /usr/local/bin/
install -m755 target/release/wrepo   /usr/local/bin/
```

Three binaries, three roles:

| Tool | Role |
|------|------|
| `wbuild` | Build parts from `plan.toml` |
| `wrepo` | Manage the part index and sources |
| `wright` | Install, remove, upgrade, query installed parts |

## Mental Model

Wright uses a single metaphor throughout the project: treat the computer as the Ship of Theseus while it is still sailing.

- A `plan.toml` is the blueprint for one replacement **part**.
- A built `.wright.tar.zst` is the finished **part**.
- `wrepo` catalogs finished parts in a repository.
- `wright` swaps parts onto the live system.

If you keep that pipeline in mind, most command names become self-explanatory. See [terminology.md](terminology.md) for the full glossary.

## Configuration

Wright works with no config file. To customize, create `/etc/wright/wright.toml`. See [configuration.md](configuration.md).

## Your First Part

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

`staging` installs files into `${PART_DIR}`. The default pipeline then runs a
final `fabricate` stage before Wright validates and archives the resulting
part; most simple plans can leave `fabricate` undefined.

### Build, index, install

```bash
wbuild run plans/hello                                  # build
wrepo sync                                              # index
wright install hello                                    # install by name
```

Or use the shortcut to build and install in one step:

```bash
wbuild run -i plans/hello                               # build + install
```

### Verify and remove

```bash
wright query hello           # show part info
wright files hello           # list installed files
wright verify hello          # check file integrity
wright remove hello          # uninstall
```

## Next Steps

- [Writing Plans](writing-plans.md) — complete plan.toml reference
- [CLI Reference](cli-reference.md) — all commands and flags
- [Usage Guide](usage.md) — build options, assemblies, tool coordination
- [Repositories](repositories.md) — indexing, sources, multi-repo setups
- [Configuration](configuration.md) — customize wright's behavior
