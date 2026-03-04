# Cookbook

Practical recipes for common build scenarios.

## Bootstrapping a New System (First Import)

When deploying wright onto a fresh LFS-based system, the core packages (glibc,
gcc, binutils, linux, etc.) are already installed but unknown to wright's
database. Any package whose `plan.toml` lists them as dependencies will fail
with an unresolved dependency error until they are registered.

Use `wright assume` to seed the database with the packages that already exist:

```sh
wright assume glibc 2.41
wright assume gcc 14.2.0
wright assume binutils 2.43
wright assume linux 6.12.0
wright assume bash 5.2
wright assume coreutils 9.5
```

After seeding, install packages normally — dependency checks will pass:

```sh
wright install man-db-2.12.1-1.wright.tar.zst
wright install python-3.13.0-1.wright.tar.zst
```

Assumed packages appear with an `[external]` tag in `wright list`:

```
bash 5.2 [external]
binutils 2.43 [external]
coreutils 9.5 [external]
gcc 14.2.0 [external]
glibc 2.41 [external]
linux 6.12.0 [external]
man-db 2.12.1-1 (x86_64)
python 3.13.0-1 (x86_64)
```

Once you have a wright-built package ready to replace a stub, simply install
it — the assumed record is replaced automatically:

```sh
wright install glibc-2.41-1.wright.tar.zst
```

After that, `wright list` will show the fully managed package entry and
`wright verify glibc` will check its file integrity as normal.

To remove an assumed record without installing a replacement, use `unassume`:

```sh
wright unassume glibc
```

---

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

[lifecycle.staging]
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

When tuning a stage without re-extracting sources every time, use `--stage` to
run specific stages against the existing build tree:

```bash
# Full first build (extracts, configures, compiles, packages)
wbuild run mypkg

# Edit lifecycle.staging in plan.toml, then re-run only staging:
wbuild run mypkg --stage staging
```

To iterate on a subset of stages and inspect the result:

```bash
wbuild run mypkg --stage configure
# Inspect $SRC_DIR (e.g. /tmp/wright-build/mypkg-1.0/src/) manually
wbuild run mypkg --stage compile
wbuild run mypkg --stage staging

# Or run compile and staging together in one command:
wbuild run mypkg --stage compile --stage staging
```

To skip the `check` stage (e.g. tests are slow or broken upstream):

```bash
wbuild run mypkg --stage prepare --stage configure --stage compile --stage staging
```

Or more concisely, run the full pipeline but skip `check` by doing a full build
and using `--stage` to re-run only the stages you need after a prior full
configure+compile:

```bash
wbuild run mypkg --stage compile --stage staging
```

---

## Multi-Package Output

A common pattern: build produces both runtime files and development headers.
Separate them so users who only need the library don't pull in headers.

```toml
[plan]
name    = "zlib"
version = "1.3.1"
release = 1
# ...

[lifecycle.staging]
script = "make DESTDIR=$PKG_DIR install"

[lifecycle.package.zlib-devel]
description = "Development files for zlib"
script = """
install -Dm644 ${BUILD_DIR}/zlib.h ${PKG_DIR}/usr/include/zlib.h
install -Dm644 ${BUILD_DIR}/zconf.h ${PKG_DIR}/usr/include/zconf.h
install -Dm644 ${BUILD_DIR}/libz.a ${PKG_DIR}/usr/lib/libz.a
install -Dm644 ${BUILD_DIR}/zlib.pc ${PKG_DIR}/usr/lib/pkgconfig/zlib.pc
"""
```

Each sub-package declared via `[lifecycle.package.<name>]` produces its own
archive. Sub-packages can define `description`, `script`, `hooks.*`, `backup`,
and `dependencies`.

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

## Removing Packages and Cleaning Up Dependencies

Remove a package and its orphan dependencies (auto-installed deps no longer needed):

```bash
wright remove --cascade nginx
```

List orphan packages (auto-installed dependencies that nothing depends on anymore):

```bash
wright list --orphans
```

If you explicitly install a package that was previously pulled in as a dependency,
it gets promoted to "explicit" and won't be removed by `--cascade`:

```bash
# pcre was auto-installed as a dependency of nginx
wright install pcre-8.45-1-x86_64.wright.tar.zst
# pcre is now "explicit" — cascade won't touch it
```

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

[lifecycle.staging]
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

[lifecycle.staging]
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

[lifecycle.staging]
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

[lifecycle.staging]
script = "install -Dm755 ${BUILD_DIR}/hugo ${PKG_DIR}/usr/bin/hugo"
```
