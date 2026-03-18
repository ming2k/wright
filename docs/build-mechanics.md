# Build Mechanics

This page explains what happens on disk when `wbuild run` executes: the build
directory layout, log files, source cache, build cache, and output archives.
Understanding these layers makes it easier to debug failures and reason about
when work is skipped or repeated.

## Build Directory Layout

Each part gets its own working directory under `build_dir`
(default `/var/tmp/wright-build`):

```
<build_dir>/<name>-<version>/
├── src/            # Extracted source tree (BUILD_DIR points here or a subdir)
├── pkg/            # Main part staging root ($PART_DIR)
├── log/            # Per-stage log files
└── .wright_script* # Temporary build script (auto-cleaned on next run)
```

`src/` is the dockyard's `/build` mount. `pkg/` is `/output`. By default,
`pkg/` and `log/` are recreated clean at the start of every build. `src/` is
**reused** when the build key has not changed (same version, sources, and
lifecycle scripts), enabling incremental builds — the fetch/verify/extract
steps are skipped entirely. When the build key changes (e.g. a version bump),
`src/` is cleaned and sources are re-extracted automatically. `--clean`
always removes the entire working directory including `src/`.

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
├── staging.log
└── ...
```

Each file contains:

```
=== Stage: compile ===
=== Exit code: 0 ===
=== Duration: 42.3s ===
=== Working dir: /var/tmp/wright-build/zlib-1.3.1/src/zlib-1.3.1 ===

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

### Directory lifecycle rules

| Operation | `src/` | `pkg/` | `log/` |
|-----------|:------:|:------:|:------:|
| Full build (key match) | **preserved** | recreated | recreated |
| Full build (key mismatch) | recreated | recreated | recreated |
| `--stage=<s>` | preserved | recreated | recreated |
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

The filename is prefixed with the part name to avoid collisions between
plans that use similarly-named upstream archives (e.g. two parts both
fetching `v1.0.tar.gz` from different projects).

Before extraction, each source is verified against its `sha256` checksum from
`plan.toml`. If the cached file fails verification, it is deleted and
re-downloaded. Local path sources use `"SKIP"` as their checksum and bypass
verification.

## Build Cache

After a successful full build, Wright saves a build cache so the part can
be skipped on future runs without re-compiling:

```
<cache_dir>/builds/
└── zlib-<build_key>.tar.zst
```

The build key is a SHA-256 hash of:

- Part name, version, and release number
- All source URIs and their expected checksums
- All lifecycle stage scripts and executor names
- Global `CFLAGS` and `CXXFLAGS` from `wright.toml`

If any of these change, the key changes and the cache is a miss — the part
rebuilds from scratch.

### What the build cache stores

The cache archive contains `pkg/` and `log/` directories.
`src/` is **not** cached to keep the archive compact. On a cache hit, Wright
restores these directories and skips the entire build pipeline.

### When the cache is bypassed or cleared

| Situation | Cache entry | Cache read | Cache write |
|-----------|:-----------:|:----------:|:-----------:|
| Normal build | kept | ✓ | ✓ |
| `--force` | kept | ✗ | ✓ |
| `--clean` | **deleted** | ✗ | ✓ |
| `--clean --force` | **deleted** | ✗ | ✓ |
| `--stage=<s>` | kept | ✗ | ✗ |
| Bootstrap (MVP first pass) | kept | ✗ | ✗ |

Bootstrap passes are intentionally incomplete builds — caching them would
produce a broken archive that a later full pass would have to overwrite anyway.

## FHS Validation

After the `fabricate` stage completes (or after `staging` for legacy plans that
do not define any fabricate hook or stage), Wright validates every file and
symlink in `$PART_DIR` against the distribution's FHS whitelist before creating
the archive. This catches silent packaging mistakes
— such as forgetting `--prefix=/usr` — at build time with a clear error:

```
validation error: part 'foo': file '/bin/foo' violates FHS — install to /usr/bin
```

Allowed install paths: `/usr/{bin,lib,lib64,share,include,libexec,libdata}`,
`/etc/`, `/var/`, `/opt/`, `/boot/`.

Absolute symlink targets are also validated. Relative symlink targets (the common
case for versioned `.so` links) are not checked.

To bypass the check for a part that intentionally deviates from the standard
layout, set `skip_fhs_check = true` in `[options]`:

```toml
[options]
skip_fhs_check = true
```

## Output Archives (Components)

After a successful build the part is packed into an archive and placed in
`components_dir` (default `/var/lib/wright/components`):

```
<components_dir>/
├── zlib-1.3.1-1-x86_64.wright.tar.zst
├── zlib-devel-1.3.1-1-x86_64.wright.tar.zst   # sub-part
└── ...
```

Archive filename format: `<name>-<version>-<release>-<arch>.wright.tar.zst`

### Skip condition

If the archive (and all sub-part archives) already exist in `components_dir`,
the build is skipped entirely — the source cache and build cache are not even
consulted. Use `--force` to override this and rebuild regardless.

### What the archive contains

The archive is created from the staged root (`pkg/`) after the final
`fabricate` phase and records the full part metadata (name, version,
dependencies, file list) for the installer. Sub-parts each get their own
archive produced by their `script`.

## Flag Quick Reference

| Flag | Source cache | Build cache | Output archive | `src/` | `pkg/` / `log/` |
|------|:---:|:---:|:---:|:---:|:---:|
| (default) | reuse | reuse | skip if exists | reuse if key matches | recreated |
| `--force` | reuse | bypass read, overwrite | overwrite | reuse if key matches | recreated |
| `--clean` | reuse | **delete + rebuild** | skip if exists | **deleted** | recreated |
| `--clean --force` | reuse | **delete + rebuild** | overwrite | **deleted** | recreated |
| `--stage=<s>` | reuse | bypass | skip | preserved | recreated |

`--clean` and `--force` address orthogonal concerns and compose naturally:
- `--clean` — invalidate the build cache **and** force a clean `src/` re-extraction
- `--force` — bypass the output archive skip check (always produce a new archive)
- `--clean --force` — "start completely from scratch": clear cache, re-extract sources, and always write a new archive

### Incremental builds

By default, `src/` is preserved across builds when the **build key** has not
changed. This allows plan authors to write lifecycle scripts that support
incremental compilation (e.g. `make` without `make clean` first). The
fetch/verify/extract steps are skipped entirely when `src/` is reused.

When the build key changes — because the version, sources, or lifecycle scripts
were modified — `src/` is automatically cleaned and sources are re-extracted.
To force a clean re-extraction without changing the plan, use `--clean`.
