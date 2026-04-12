# Build Mechanics

This page explains what happens on disk when `wright build` executes: the build
directory layout, log files, source cache, build cache, and output parts.
Understanding these layers makes it easier to debug failures and reason about
when work is skipped or repeated.

## Three On-Disk Layers

`wright build` uses three different storage layers that are easy to confuse:

| Location | Purpose | Typical contents | Lifecycle |
|----------|---------|------------------|-----------|
| `build_dir` (default `/var/tmp/wright-build`) | Live working directory for a build | `src/`, `pkg/`, `log/` | Scratch/workspace; may be deleted and recreated freely |
| `<cache_dir>/sources/` | Reusable source input cache | Downloaded tarballs, zip files, bare git repos | Persistent cache across builds |
| `<cache_dir>/builds/` | Reusable build-result cache | `<name>-<build_key>.tar.zst` snapshots containing built output and logs | Persistent cache across builds |

The key distinction is that `build_dir` is an unpacked workspace, while
`cache_dir/builds` is a formal cache entry. A cache hit can restore `pkg/` and
`log/` even if the previous working directory under `build_dir` was deleted.

### How the three layers relate

Quick rule:

- `build cache` decides whether Wright can skip the build entirely.
- `build_dir/src/` decides whether Wright can reuse the previous unpacked source tree.
- `source cache` decides whether Wright must re-download or re-copy source inputs.

Execution order:

1. Check `build cache`
2. If missed, check whether `build_dir/src/` is reusable
3. If not reusable, fetch/extract from `source cache`

`build cache` and `build_dir/src/` share the same build key, but store different
state: `build_dir/src/` is mutable workspace state; `build cache` is a compact
result snapshot and does **not** include `src/`.

## Build Directory Layout

Each part gets its own working directory under `build_dir`
(default `/var/tmp/wright-build`):

```
<build_dir>/<name>-<version>/
├── src/      # Extracted source tree (BUILD_DIR points here or a subdir)
├── pkg/      # Main part staging root ($PART_DIR)
├── log/      # Per-stage log files
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
| Cache hit | recreated empty | restored | restored |

On a build-cache hit, Wright recreates the working directories first, then
extracts the cached snapshot into `build_root`. Because `src/` is not part of
that snapshot, the resulting `src/` directory exists but contains no restored
source tree.

## Source Cache

Downloaded sources are stored permanently in `<cache_dir>/sources/` and reused
across builds:

```
<cache_dir>/sources/
├── zlib-zlib-1.3.1.tar.gz     # <pkg_name>-<upstream_basename>
├── gcc-gcc-14.2.0.tar.xz
└── git/
  └── linux            # bare git repos
```

The filename is prefixed with the part name to avoid collisions between
plans that use similarly-named upstream parts (e.g. two parts both
fetching `v1.0.tar.gz` from different projects).

Before extraction, each source is verified against its `sha256` checksum from
`plan.toml`. If the cached file fails verification, it is deleted and
re-downloaded. Local path sources use `"SKIP"` as their checksum and bypass
verification.

The source cache is only consulted when Wright needs to materialize `src/`
again. If `src/` is reusable, or if a build-cache hit skips the pipeline, it is
not used in that run.

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

If any of these change, the key changes and the cache is a miss — the part
rebuilds from scratch.

### What the build cache stores

The cache part contains `pkg/` and `log/` directories.
`src/` is **not** cached to keep the part compact. On a cache hit, Wright
restores these directories and skips the entire build pipeline.

For multi-part plans, `pkg-*` sub-part directories are also included in the
cache entry.

### Why this exists when `build_dir` already exists

`build_dir` is primarily for live build state and debugging. It keeps the
extracted source tree and the logs from the last run, but it is not the build
cache interface. Wright still writes `cache_dir/builds/<name>-<build_key>.tar.zst`
because:

- the working directory may be removed manually or by `--clean`
- `src/` is intentionally excluded from the build cache to keep cache entries smaller
- the cache key provides a precise "can this build be reused?" decision
- restoring a compact part is more predictable than relying on an old working tree

If you want to inspect source trees or rerun a stage manually, look in
`build_dir`. If you want to understand why a later build skipped recompilation,
look in `cache_dir/builds`.

### Build Cache vs Output Part

| Item | Build cache | `.wright.tar.zst` |
|------|-------------|-------------------|
| Purpose | Internal reuse | Distribution / install |
| Produced when | After lifecycle succeeds | After FHS validation and part creation |
| Contains | `pkg/`, `log/`, `pkg-*` | Packaged payload plus `.PARTINFO`, `.FILELIST`, optional `.HOOKS` |
| Includes `src/` | No | No |
| Stable public format | No | Yes |

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
produce a broken part that a later full pass would have to overwrite anyway.

## FHS Validation

After the final output stage completes (`fabricate`), Wright validates every
file and symlink in `$PART_DIR` against the distribution's FHS whitelist before
creating the part. This catches silent packaging mistakes
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

## Output parts (Components)

After a successful build the part is packed into an part and placed in
`components_dir` (default `/var/lib/wright/components`):

```
<components_dir>/
├── zlib-1.3.1-1-x86_64.wright.tar.zst
├── zlib-devel-1.3.1-1-x86_64.wright.tar.zst  # sub-part
└── ...
```

part filename format: `<name>-<version>-<release>-<arch>.wright.tar.zst`

### Skip condition

If the part (and all sub-part parts) already exist in `components_dir`,
the build is skipped entirely — the source cache and build cache are not even
consulted. Use `--force` to override this and rebuild regardless.

### What the part contains

The part is created from the staged root (`pkg/`) after the final
`fabricate` phase and records the full part metadata (name, version,
dependencies, file list) for the installer. Sub-parts each get their own
part produced by their `script`.

## Flag Quick Reference

| Flag | Source cache | Build cache | Output part | `src/` | `pkg/` / `log/` |
|------|:---:|:---:|:---:|:---:|:---:|
| (default) | reuse | reuse | skip if exists | reuse if key matches | recreated |
| `--force` | reuse | bypass read, overwrite | overwrite | reuse if key matches | recreated |
| `--clean` | reuse | **delete + rebuild** | skip if exists | **deleted** | recreated |
| `--clean --force` | reuse | **delete + rebuild** | overwrite | **deleted** | recreated |
| `--stage=<s>` | reuse | bypass | skip | preserved | recreated |

`--clean` and `--force` address orthogonal concerns and compose naturally:
- `--clean` — invalidate the build cache **and** force a clean `src/` re-extraction
- `--force` — bypass the output part skip check (always produce a new part)
- `--clean --force` — "start completely from scratch": clear cache, re-extract sources, and always write a new part

### Incremental builds

By default, `src/` is preserved across builds when the **build key** has not
changed. This allows plan authors to write lifecycle scripts that support
incremental compilation (e.g. `make` without `make clean` first). The
fetch/verify/extract steps are skipped entirely when `src/` is reused.

When the build key changes — because the version, sources, or lifecycle scripts
were modified — `src/` is automatically cleaned and sources are re-extracted.
To force a clean re-extraction without changing the plan, use `--clean`.
