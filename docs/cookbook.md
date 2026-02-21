# Cookbook

Practical recipes for common build scenarios.

## Building a Simple Package

Minimal `plan.toml` for a C library (`zlib`):

```toml
[plan]
name    = "zlib"
version = "1.3.1"
release = 1
description = "Compression library"
license = "Zlib"
arch    = "x86_64"
url     = "https://zlib.net"

[sources]
uris   = ["https://zlib.net/zlib-1.3.1.tar.gz"]
sha256 = ["9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"]

[lifecycle.configure]
script = "./configure --prefix=/usr"

[lifecycle.compile]
script = "make"

[lifecycle.package]
script = "make DESTDIR=$PKG_DIR install"
```

Fetch checksums automatically:

```bash
wbuild checksum zlib
```

Build:

```bash
wbuild run zlib
```

Build and install immediately:

```bash
wbuild run zlib -i
```

---

## Iterating on a Build Script

When tuning a stage without re-extracting sources every time, use `--only` to
run a single stage against the existing build tree:

```bash
# Full first build (extracts, configures, compiles, packages)
wbuild run mypkg

# Edit lifecycle.package in plan.toml, then re-run only packaging:
wbuild run mypkg --only package
```

To iterate up to a specific stage and inspect the result:

```bash
wbuild run mypkg --until configure
# Inspect $SRC_DIR (e.g. /tmp/wright-build/mypkg-1.0/src/) manually
wbuild run mypkg --only compile
wbuild run mypkg --only package
```

---

## Splitting a Package

A common pattern: build produces both runtime files and development headers.
Separate them so users who only need the library don't pull in headers.

```toml
[plan]
name    = "zlib"
version = "1.3.1"
release = 1
# ...

[lifecycle.package]
script = """
make DESTDIR=$PKG_DIR install
# Strip headers and static lib; they go into zlib-devel
rm -rf $PKG_DIR/usr/include
rm -f  $PKG_DIR/usr/lib/libz.a
"""

[split.zlib-devel]
description = "Development files for zlib"

[split.zlib-devel.lifecycle.package]
script = """
make DESTDIR=$MAIN_PKG_DIR install
# Copy only the devel files into this split's PKG_DIR
install -dm755 $PKG_DIR/usr/include $PKG_DIR/usr/lib
cp -a $MAIN_PKG_DIR/usr/include/zlib*.h $PKG_DIR/usr/include/
cp -a $MAIN_PKG_DIR/usr/lib/libz.a      $PKG_DIR/usr/lib/
"""
```

`$MAIN_PKG_DIR` is mounted at `/main-pkg` inside the dockyard and points to the
main package's staging root. Split packages run their `package` stage after the
main package stage completes.

---

## Handling a Circular Dependency (MVP / Bootstrap)

Some packages require themselves or each other to build (e.g. a compiler that
compiles itself). Wright resolves this with a two-pass build:

1. **MVP pass** — build the package with a reduced dependency set (no cyclic dep)
2. **Full pass** — rebuild with all dependencies, now that the cycle is broken

```toml
[plan]
name    = "gcc"
version = "14.2.0"
# ...

[dependencies]
build = ["binutils", "glibc", "gcc"]   # gcc needs itself — cycle!

[mvp.dependencies]
build = ["binutils", "glibc"]          # MVP: build without gcc in deps
```

Wright detects the cycle automatically and schedules:

```
Construction Plan:
  [MVP]          gcc          ← first pass, no gcc dep
  [NEW]          binutils
  [FULL]         gcc          ← second pass, full deps
```

To test the MVP pass explicitly without a cycle present:

```bash
wbuild run gcc --mvp
```

To inspect what cycles exist and which packages are MVP candidates:

```bash
wbuild check gcc binutils glibc
```

---

## Building a Dependency Chain

Build a package and all of its missing upstream dependencies:

```bash
# Default: build gtk4 + auto-resolve any missing build/link deps
wbuild run gtk4
```

Build only the missing deps, not gtk4 itself (pre-stage before the main build):

```bash
wbuild run gtk4 --deps
```

Build everything — deps, the package, and downstream link dependents:

```bash
wbuild run gtk4 --self --deps --dependents
```

---

## Rebuilding After a Library Update

A library's ABI changed. Rebuild everything that links against it:

```bash
# Update the library, then cascade to all link dependents
wbuild run libfoo --self --dependents
```

The scheduler labels affected packages as `[LINK-REBUILD]` in the Construction
Plan. To also catch runtime and build dependents (full reverse cascade):

```bash
wbuild run libfoo --self --dependents -R
```

To limit how deep the cascade goes:

```bash
wbuild run libfoo --self --dependents --depth 2
```

---

## Building an Assembly

Assemblies are named groups of packages. Build all packages in `@base`:

```bash
wbuild run @base
```

Build multiple assemblies and extra individual packages in one invocation:

```bash
wbuild run @base @devel mypackage
```

---

## Force-Rebuild Everything from Source

Useful when global flags change (e.g. new `CFLAGS`) and you want to rebuild
all packages in an assembly:

```bash
wbuild run @base --force
```

`--force` bypasses both the archive skip check and the build cache for every
package in the set.

---

## Inspecting a Package's Dependency Tree

Print the full build-time dependency tree for a plan:

```bash
wbuild deps gtk4
```

Limit depth:

```bash
wbuild deps gtk4 --depth 2
```

---

## Custom Executor (Python Build Script)

For packages whose build system is easier to drive with Python:

```toml
[lifecycle.configure]
executor = "python"
script = """
import subprocess, os
subprocess.run(["python", "setup.py", "configure"], check=True)
"""
```

Executor definitions live in `executors_dir` (default `/etc/wright/executors`).
See [configuration.md](configuration.md) for the executor format.

---

## Rust / Cargo Package (Vendored — strict dockyard)

The preferred approach: vendor crates in the source tree so the build is fully
offline and runs under the default `strict` dockyard.

```toml
[plan]
name    = "ripgrep"
version = "14.1.1"
release = 1
description = "Fast line-oriented search tool"
license = "MIT OR Unlicense"
arch    = "x86_64"

[sources]
# Source tarball already contains a vendor/ directory generated by `cargo vendor`
uris   = ["https://example.org/ripgrep-14.1.1-vendored.tar.gz"]
sha256 = ["<sha256>"]

[lifecycle.compile]
# dockyard defaults to "strict" — no network access needed thanks to vendoring
script = """
export CARGO_HOME=${SRC_DIR}/.cargo-home
cat > ${SRC_DIR}/.cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF
cargo build --release --offline
"""

[lifecycle.package]
script = """
install -Dm755 ${SRC_DIR}/target/release/rg ${PKG_DIR}/usr/bin/rg
"""
```

If the upstream tarball does not include a `vendor/` directory, generate one
locally and repack:

```bash
cargo vendor vendor/
tar czf ripgrep-14.1.1-vendored.tar.gz ripgrep-14.1.1/
```

---

## Rust / Cargo Package (Network — relaxed dockyard)

When vendoring is impractical (e.g. bootstrapping), allow network access by
setting `dockyard = "relaxed"` on the compile stage. The build gets a private
mount and PID namespace but retains host network access.

```toml
[lifecycle.compile]
dockyard = "relaxed"
script = """
export CARGO_HOME=${SRC_DIR}/.cargo-home
cargo build --release
"""

[lifecycle.package]
script = """
install -Dm755 ${SRC_DIR}/target/release/rg ${PKG_DIR}/usr/bin/rg
"""
```

---

## Go Package (Vendored — strict dockyard)

Run `go mod vendor` before packaging the source tarball so the build is
offline under `strict`.

```toml
[plan]
name    = "hugo"
version = "0.136.0"
release = 1
description = "Fast static site generator"
license = "Apache-2.0"
arch    = "x86_64"

[sources]
# Tarball includes vendor/ generated by `go mod vendor`
uris   = ["https://example.org/hugo-0.136.0-vendored.tar.gz"]
sha256 = ["<sha256>"]

[lifecycle.compile]
# strict dockyard — no network; -mod=vendor forces Go to use vendor/
script = """
cd ${BUILD_DIR}
go build -mod=vendor -o hugo .
"""

[lifecycle.package]
script = """
install -Dm755 ${BUILD_DIR}/hugo ${PKG_DIR}/usr/bin/hugo
"""
```

---

## Go Package (Network — relaxed dockyard)

Without a vendor directory, Go downloads modules from proxy.golang.org at build
time. Use `relaxed` so the network namespace is shared with the host.

```toml
[lifecycle.compile]
dockyard = "relaxed"
script = """
cd ${BUILD_DIR}
export GOPATH=${SRC_DIR}/.gopath
export GOMODCACHE=${SRC_DIR}/.gopath/pkg/mod
go build -o hugo .
"""

[lifecycle.package]
script = "install -Dm755 ${BUILD_DIR}/hugo ${PKG_DIR}/usr/bin/hugo"
```
