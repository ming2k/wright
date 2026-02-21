# Build Mechanics

This page explains what happens on disk when `wbuild run` executes: the build
directory layout, log files, source cache, build cache, and output archives.
Understanding these layers makes it easier to debug failures and reason about
when work is skipped or repeated.

## Build Directory Layout

Each package gets its own working directory under `build_dir`
(default `/tmp/wright-build`):

```
<build_dir>/<name>-<version>/
├── src/            # Extracted source tree (BUILD_DIR points here or a subdir)
├── pkg/            # Main package staging root ($PKG_DIR)
├── pkg-<split>/    # One directory per split package
├── log/            # Per-stage log files
└── .wright_script* # Temporary build script (auto-cleaned on next run)
```

`src/` is the dockyard's `/build` mount. `pkg/` is `/output`. The directory is
recreated clean at the start of every full build run. `--only` recreates `pkg/`
and `log/` but leaves `src/` intact so the previous extraction is reused.

### BUILD_DIR auto-detection

If after extraction `src/` contains exactly one subdirectory, `$BUILD_DIR` is
set to that subdirectory (the common case for tarballs that unpack into
`<name>-<version>/`). Otherwise `$BUILD_DIR` equals `$SRC_DIR`.

## Log Files

Every lifecycle stage writes a log file under `log/`:

```
<build_dir>/<name>-<version>/log/
├── configure.log
├── compile.log
├── package.log
├── package-<split>.log   # one per split package
└── ...
```

Each file contains:

```
=== Stage: compile ===
=== Exit code: 0 ===
=== Duration: 42.3s ===
=== Working dir: /tmp/wright-build/zlib-1.3.1/src/zlib-1.3.1 ===

--- script ---
make

--- stdout ---
...

--- stderr ---
...
```

Log files are **always written**, regardless of whether `-v` is set. `-v`
additionally echoes output to the terminal in real time.

### On failure

When a stage exits non-zero, the last 40 lines of stderr (or stdout if stderr
is empty) are printed to the terminal directly. The full output is always in
the log file. Logs from the failed run are **preserved** — they are only
overwritten on the next build attempt.

### Log recreation rules

| Operation | `src/` | `pkg/` | `log/` |
|-----------|:------:|:------:|:------:|
| Full build | recreated | recreated | recreated |
| `--only <stage>` | preserved | recreated | recreated |
| `--clean` then build | deleted first | recreated | recreated |
| Cache hit | recreated from cache | restored | restored |

## Source Cache

Downloaded sources are stored permanently in `<cache_dir>/sources/` and reused
across builds:

```
<cache_dir>/sources/
├── zlib-zlib-1.3.1.tar.gz          # <pkg_name>-<upstream_basename>
├── gcc-gcc-14.2.0.tar.xz
└── git/
    └── linux                        # bare git repos
```

The filename is prefixed with the package name to avoid collisions between
packages that use similarly-named upstream archives (e.g. two packages both
fetching `v1.0.tar.gz` from different projects).

Before extraction, each source is verified against its `sha256` checksum from
`plan.toml`. If the cached file fails verification, it is deleted and
re-downloaded. Local path sources use `"SKIP"` as their checksum and bypass
verification.

## Build Cache

After a successful full build, Wright saves a build cache so the package can
be skipped on future runs without re-compiling:

```
<cache_dir>/builds/
└── zlib-<build_key>.tar.zst
```

The build key is a SHA-256 hash of:

- Package name, version, and release number
- All source URIs and their expected checksums
- All lifecycle stage scripts and executor names
- Global `CFLAGS` and `CXXFLAGS` from `wright.toml`

If any of these change, the key changes and the cache is a miss — the package
rebuilds from scratch.

### What the build cache stores

The cache archive contains `pkg/`, `log/`, and any `pkg-<split>/` directories.
`src/` is **not** cached to keep the archive compact. On a cache hit, Wright
restores these directories and skips the entire build pipeline.

### When the cache is bypassed or cleared

| Situation | Cache entry | Cache read | Cache write |
|-----------|:-----------:|:----------:|:-----------:|
| Normal build | kept | ✓ | ✓ |
| `--force` | kept | ✗ | ✓ |
| `--clean` | **deleted** | ✗ | ✓ |
| `--clean --force` | **deleted** | ✗ | ✓ |
| `--only <stage>` | kept | ✗ | ✗ |
| `--until <stage>` | kept | ✗ | ✗ |
| Bootstrap (MVP first pass) | kept | ✗ | ✗ |

Bootstrap passes are intentionally incomplete builds — caching them would
produce a broken archive that a later full pass would have to overwrite anyway.

## Output Archives (Components)

After a successful build the package is packed into an archive and placed in
`components_dir` (default `/var/lib/wright/components`):

```
<components_dir>/
├── zlib-1.3.1-1-x86_64.wright.tar.zst
├── zlib-devel-1.3.1-1-x86_64.wright.tar.zst   # split package
└── ...
```

Archive filename format: `<name>-<version>-<release>-<arch>.wright.tar.zst`

### Skip condition

If the archive (and all split archives) already exist in `components_dir`,
the build is skipped entirely — the source cache and build cache are not even
consulted. Use `--force` to override this and rebuild regardless.

### What the archive contains

The archive is created from the staging root (`pkg/`) and records the full
package metadata (name, version, dependencies, file list) for the installer.
Split packages each get their own archive from their `pkg-<split>/` directory.

## Flag Quick Reference

| Flag | Source cache | Build cache | Output archive | Working dir |
|------|:---:|:---:|:---:|:---:|
| (default) | reuse | reuse | skip if exists | always recreated |
| `--force` | reuse | bypass read, overwrite | overwrite | always recreated |
| `--clean` | reuse | **delete + rebuild** | skip if exists | delete then recreated |
| `--clean --force` | reuse | **delete + rebuild** | overwrite | delete then recreated |
| `--only` | reuse | bypass | skip | keep `src/` |
| `--until` | reuse | bypass | skip | always recreated |

`--clean` and `--force` address orthogonal concerns and compose naturally:
- `--clean` — invalidate the build cache (force full recompile)
- `--force` — bypass the output archive skip check (always produce a new archive)
- `--clean --force` — "start completely from scratch": clear cache and always write a new archive
