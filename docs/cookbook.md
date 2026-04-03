# Cookbook

Practical recipes for common build scenarios.

## Bootstrapping a New System (First Import)

When deploying wright onto a fresh LFS-based system, the core parts (glibc,
gcc, binutils, linux, etc.) are already installed but unknown to wright's
database. Any part whose `plan.toml` lists them as dependencies will fail
with an unresolved dependency error until they are registered.

Use `wright assume` to seed the database with the parts that already exist:

```sh
wright assume glibc 2.41
wright assume gcc 14.2.0
wright assume binutils 2.43
wright assume linux 6.12.0
wright assume bash 5.2
wright assume coreutils 9.5
```

After seeding, install parts normally — dependency checks will pass:

```sh
wright install man-db-2.12.1-1.wright.tar.zst
wright install python-3.13.0-1.wright.tar.zst
```

Assumed parts appear with an `[external]` tag in `wright list`:

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

Once you have a wright-built part ready to replace a stub, simply install
it — the assumed record is replaced automatically:

```sh
wright install glibc-2.41-1.wright.tar.zst
```

After that, `wright list` will show the fully managed part entry and
`wright verify glibc` will check its file integrity as normal.

To remove an assumed record without installing a replacement, use `unassume`:

```sh
wright unassume glibc
```

---

## Building a Simple Part

Minimal `plan.toml` for a C library (`zlib`):

```toml
name    = "zlib"
version = "1.3.1"
release = 1
description = "Compression library"
license = "Zlib"
arch    = "x86_64"
url     = "https://zlib.net"

[[sources]]
uri = "https://zlib.net/zlib-1.3.1.tar.gz"
sha256 = "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"

[lifecycle.configure]
script = "./configure --prefix=/usr"

[lifecycle.compile]
script = "make"

[lifecycle.staging]
script = "make DESTDIR=$PART_DIR install"
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
# Full first build (extracts, configures, compiles, fabricates parts)
wbuild run mypkg

# Edit lifecycle.staging in plan.toml, then re-run the output phases:
wbuild run mypkg --stage=staging --stage=fabricate
```

To iterate on a subset of stages and inspect the result:

```bash
wbuild run mypkg --stage=configure
# Inspect $SRC_DIR (e.g. /var/tmp/wright-build/mypkg-1.0/src/) manually
wbuild run mypkg --stage=compile
wbuild run mypkg --stage=staging --stage=fabricate

# Or run compile plus the output phases together in one command:
wbuild run mypkg --stage=compile --stage=staging --stage=fabricate
```

To skip the `check` stage (e.g. tests are slow or broken upstream):

```bash
wbuild run mypkg --stage=prepare --stage=configure --stage=compile --stage=staging --stage=fabricate
```

Or more concisely, run the full pipeline but skip `check` by doing a full build
and using `--stage` to re-run only the stages you need after a prior full
configure+compile:

```bash
wbuild run mypkg --stage=compile --stage=staging --stage=fabricate
```

---

## Multi-Package Output

A common pattern: build produces both runtime files and development headers.
Separate them so users who only need the library don't pull in headers.

```toml
name    = "zlib"
version = "1.3.1"
release = 1
# ...

[lifecycle.staging]
script = "make DESTDIR=$PART_DIR install"

[output.zlib-devel]
description = "Development files for zlib"
script = """
install -Dm644 ${BUILD_DIR}/zlib.h ${PART_DIR}/usr/include/zlib.h
install -Dm644 ${BUILD_DIR}/zconf.h ${PART_DIR}/usr/include/zconf.h
install -Dm644 ${BUILD_DIR}/libz.a ${PART_DIR}/usr/lib/libz.a
install -Dm644 ${BUILD_DIR}/zlib.pc ${PART_DIR}/usr/lib/pkgconfig/zlib.pc
"""
```

Each sub-part declared via `[output.<name>]` produces its own
archive. Sub-parts can define `description`, `script`, `hooks.*`, `backup`,
and `dependencies`.

---

## Handling a Circular Dependency (MVP / Bootstrap)

Some parts require themselves or each other to build (e.g. a compiler that
compiles itself). Wright resolves this with a two-pass build:

1. **MVP pass** — build the part with a reduced dependency set (no cyclic dep)
2. **Full pass** — rebuild with all dependencies, now that the cycle is broken

```toml
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
INFO Scheduling batch 0 build:mvp: gcc   ← first pass, no gcc dep
INFO Scheduling batch 0 build: binutils
INFO Scheduling batch 1 build:full: gcc  ← second pass, full deps
```

To test the MVP pass explicitly without a cycle present:

```bash
wbuild run gcc --mvp
```

To inspect what cycles exist and which parts are MVP candidates:

```bash
wbuild check gcc binutils glibc
```

---

## Building a Dependency Chain

Build a part and all of its missing upstream dependencies:

```bash
# Resolve gtk4 plus any missing build/link deps, then build
wbuild resolve gtk4 --self --deps | wbuild run
```

Build only the missing deps, not gtk4 itself (pre-stage before the main build):

```bash
wbuild resolve gtk4 --deps | wbuild run
```

Build everything — deps, the part, and downstream link dependents:

```bash
wbuild resolve gtk4 --self --deps --dependents | wbuild run -i
```

---

## Rebuilding After a Library Update

A library's ABI changed. Rebuild everything that links against it:

```bash
# Update the library, then cascade to all installed link dependents
wbuild resolve libfoo --self --dependents | wbuild run --force -i
```

The scheduler labels affected parts as `relink` in the scheduling log. To also catch runtime and build dependents (full reverse cascade):

```bash
wbuild resolve libfoo --self --dependents=all --depth=0 | wbuild run --force -i
```

To limit how deep the cascade goes:

```bash
wbuild resolve libfoo --self --dependents --depth=2 | wbuild run --force -i
```

---

## Resuming After a Failed Build

When a large cascade build fails partway through, use `--resume` to continue
without re-building parts that already succeeded:

```bash
# First run — fails on package 15 of 30:
wbuild resolve pcre2 --self --dependents --depth=0 | wbuild run --force -i
# Output: Build session: a1b2c3...  (resume with: --resume a1b2c3...)

# Resume — skips the 14 already-completed packages:
wbuild resolve pcre2 --self --dependents --depth=0 | wbuild run --resume -i
```

`--resume` tracks progress in a build session stored in the database. Each
successfully built and installed part is recorded. On resume, those parts are
skipped and the rest are rebuilt.

With `-i`, builds run against a session-local overlay sysroot and temporary
package database snapshot. Completed packages are staged into that session root
between dependency waves so later builds see a stable root state. Package
install/upgrade hooks are skipped during staging and run only when the staged
outputs are committed to host `/` at the end of a successful run.

The session hash is deterministic — running the same `wbuild resolve | wbuild run`
pipeline produces the same hash, so `--resume` auto-detects the session. You can
also pass the hash explicitly:

```bash
wbuild resolve pcre2 --self --dependents --depth=0 | wbuild run --resume a1b2c3... -i
```

Sessions are cleaned up automatically when all parts complete successfully.

---

## Building an Assembly

Assemblies are non-dependent, combinatory groupings of plans — items are
independent units bundled for convenience, not a dependency chain. Build
ordering comes from each plan's own dependency graph, not from assembly
membership. Multiple assemblies can be freely combined and overlapping
plans are deduplicated.

```bash
wbuild run @base                    # build all plans in the "base" assembly
wbuild run @base @devel mypackage   # combine assemblies and individual plans
wbuild run -i @base                 # build and install the requested assembly
wbuild resolve @base --self --deps=sync | wbuild run -i # also sync missing/outdated upstream deps
```

---

## Force-Rebuild Everything from Source

Useful when global flags change (e.g. new `CFLAGS`) and you want to rebuild
all parts in an assembly:

```bash
wbuild run @base --force
```

`--force` bypasses both the archive skip check and the build cache for every
part in the set.

---

## Removing Packages and Cleaning Up Dependencies

Remove a part and its orphan dependencies (auto-installed deps no longer needed):

```bash
wright remove --cascade nginx
```

List orphan parts (auto-installed dependencies that nothing depends on anymore):

```bash
wright list --orphans
```

If you explicitly install a part that was previously pulled in as a dependency,
its origin gets promoted to `manual` and won't be removed by `--cascade`:

```bash
# pcre was auto-installed as a dependency of nginx (origin: dependency)
wright install pcre-8.45-1-x86_64.wright.tar.zst
# pcre is now "manual" — cascade won't touch it
```

---

## Inspecting a Part's Dependency Tree

Print the full build-time dependency tree for a plan:

```bash
wbuild resolve gtk4 --tree
```

Limit depth:

```bash
wbuild resolve gtk4 --tree --depth=2
```

---

## Custom Executor (Python Build Script)

For parts whose build system is easier to drive with Python:

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

## Rust / Cargo Package (Vendored - strict dockyard)

The preferred approach: vendor crates in the source tree so the build is fully
offline and runs under the default `strict` dockyard.

```toml
name    = "ripgrep"
version = "14.1.1"
release = 1
description = "Fast line-oriented search tool"
license = "MIT OR Unlicense"
arch    = "x86_64"

[[sources]]
# Source tarball already contains a vendor/ directory generated by `cargo vendor`
uri = "https://example.org/ripgrep-14.1.1-vendored.tar.gz"
sha256 = "<sha256>"

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
install -Dm755 ${SRC_DIR}/target/release/rg ${PART_DIR}/usr/bin/rg
"""
```

If the upstream tarball does not include a `vendor/` directory, generate one
locally and repack:

```bash
cargo vendor vendor/
tar czf ripgrep-14.1.1-vendored.tar.gz ripgrep-14.1.1/
```

---

## Rust / Cargo Package (Network - relaxed dockyard)

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
install -Dm755 ${SRC_DIR}/target/release/rg ${PART_DIR}/usr/bin/rg
"""
```

---

## Go Package (Vendored - strict dockyard)

Run `go mod vendor` before packaging the source tarball so the build is
offline under `strict`.

```toml
name    = "hugo"
version = "0.136.0"
release = 1
description = "Fast static site generator"
license = "Apache-2.0"
arch    = "x86_64"

[[sources]]
# Tarball includes vendor/ generated by `go mod vendor`
uri = "https://example.org/hugo-0.136.0-vendored.tar.gz"
sha256 = "<sha256>"

[lifecycle.compile]
# strict dockyard — no network; -mod=vendor forces Go to use vendor/
script = """
cd ${BUILD_DIR}
go build -mod=vendor -o hugo .
"""

[lifecycle.staging]
script = """
install -Dm755 ${BUILD_DIR}/hugo ${PART_DIR}/usr/bin/hugo
"""
```

---

## Go Package (Network - relaxed dockyard)

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
script = "install -Dm755 ${BUILD_DIR}/hugo ${PART_DIR}/usr/bin/hugo"
```
