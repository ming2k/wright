# Build Mechanics

This page explains what happens on disk when `wright build` executes: the build
directory layout, log files, source cache, and output parts.
Understanding these layers makes it easier to debug failures and reason about
when work is skipped or repeated.

## Two On-Disk Layers

`wright build` uses two different storage layers:

| Location | Purpose | Typical contents | Lifecycle |
|----------|---------|------------------|-----------|
| `build_dir` (default `/var/tmp/wright/workshop`) | Live working directory for a build | `work/`, `output/`, `logs/` | Scratch/workspace; may be deleted and recreated freely |
| `source_dir` (default `/var/lib/wright/sources`) | Reusable source input cache | Downloaded tarballs, zip files, bare git repos | Persistent cache across builds |

### How the two layers relate

- `build_dir/<name>-<version>/work/` (or `build_dir/<name>-noversion/work/` when `version` is omitted) decides whether Wright can reuse the previous unpacked source tree.
- `source cache` decides whether Wright must re-download or re-copy source inputs.

Execution order:

1. Check whether `build_dir/<name>-<version>/work/` (or `build_dir/<name>-noversion/work/` when version is omitted) is reusable (build key match)
2. If not reusable, fetch/extract from `source cache`

## Build Directory Layout

Each part gets its own working directory under `build_dir`
(default `/var/tmp/wright/workshop`):

```
<build_dir>/<name>-<version>/¹
├── work/     # The source tree (mounted at /build in isolation)
├── output/   # Main output staging root ($PART_DIR / $MAIN_PART_DIR, mounted at /output)
├── logs/     # Per-stage log files
└── .wright_script* # Temporary build script (auto-cleaned on next run)

¹ When `version` is omitted from `plan.toml`, the directory uses `<name>-noversion`.
```

`work/` is the isolation's `/build` mount. `output/` is `/output`. By default,
`output/` and `logs/` are recreated clean at the start of every build. `work/` is
**reused** when the build key has not changed (same version, sources, and
lifecycle scripts), enabling incremental builds — the fetch/verify/extract
steps are skipped entirely. When the build key changes (e.g. a version bump),
`work/` is cleaned and sources are re-extracted automatically. `--clean`
always removes the entire working directory including `work/`.

If multiple outputs are defined in `plan.toml` (split-parts), additional
staging directories are created:

```
<build_dir>/<name>-<version>/¹
├── output/          # Main part output
├── output-<name>/   # Sub-part output (e.g. output-dev)

¹ Uses `<name>-noversion` when `version` is omitted.
└── ...
```

During sub-part staging, the main part's output is mounted read-only at
`/main-part` (and available via `${MAIN_PART_DIR}`).


set to that subdirectory (the common case for tarballs that unpack into

## Log Files

Every lifecycle stage writes a log file under `logs/`:

```
<build_dir>/<name>-<version>/logs/
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
=== Working dir: /var/tmp/wright/workshop/zlib-1.3.1/work ===

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

**Golden Standard:** Wright automatically maps internal sandbox paths back to
their corresponding variables in error messages. If a script fails, you will
see `${PART_DIR}/usr/bin` in the output instead of the internal `/output/usr/bin`
path.

### Directory lifecycle rules

| Operation | `work/` | `output/` | `logs/` |
|-----------|:------:|:------:|:------:|
| Full build (key match) | **preserved** | recreated | recreated |
| Full build (key mismatch) | recreated | recreated | recreated |
| `--stage=<s>` | preserved | recreated | recreated |
| `--clean` then build | deleted first | recreated | recreated |
| Cache hit | recreated empty | restored | restored |

On a build-cache hit, Wright recreates the working directories first, then
extracts the cached snapshot into `build_root`. Because `work/` is not part of
that snapshot, the resulting `work/` directory exists but contains no restored
source tree.

## Source Cache

Downloaded sources are stored permanently in `source_dir` and reused
across builds:

```
<source_dir>/
├── zlib-zlib-1.3.1.tar.gz     # <part_name>-<dependency_basename>
├── gcc-gcc-14.2.0.tar.xz
└── git/
  └── linux            # bare git repos
```

The filename is prefixed with the part name to avoid collisions between
plans that use similarly-named dependency parts (e.g. two parts both
fetching `v1.0.tar.gz` from different projects).

Before extraction, each source is verified against its `sha256` checksum from
`plan.toml`. If the cached file fails verification, it is deleted and
re-downloaded. Local path sources use `"SKIP"` as their checksum and bypass
verification.

The source cache is only consulted when Wright needs to materialize `work/`
again. If `work/` is reusable, it is not used in that run.

## FHS Validation

After the final staging and output slicing completes, Wright validates every
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

After a successful build the part is packed into a part file and placed in
`parts_dir` (default `/var/lib/wright/parts`):

```
<parts_dir>/
├── zlib-1.3.1-1-x86_64.wright.tar.zst
├── zlib-devel-1.3.1-1-x86_64.wright.tar.zst  # sub-part
└── ...
```

part filename format:
- With version: `<name>-<version>-<release>-<arch>.wright.tar.zst`
- Without version: `<name>-<release>-<arch>.wright.tar.zst`

### Skip condition

If the part (and all sub-part parts) already exist in `parts_dir`,
the build is skipped entirely — the source cache is not even consulted.
Use `--force` to override this and rebuild regardless.

### What the part contains

The part is created from the staged root (`output/`) after the final
output slicing phase and records the full part metadata (name, version,
dependencies, file list) for the installer. Sub-parts each get their own
part produced by their `script`.

## Flag Quick Reference

| Flag | Source cache | Output part | `work/` | `output/` / `logs/` |
|------|:---:|:---:|:---:|:---:|
| (default) | reuse | skip if exists | reuse if key matches | recreated |
| `--force` | reuse | overwrite | reuse if key matches | recreated |
| `--clean` | reuse | skip if exists | **deleted** | recreated |
| `--clean --force` | reuse | overwrite | **deleted** | recreated |
| `--stage=<s>` | reuse | skip | preserved | recreated |

`--clean` and `--force` address orthogonal concerns and compose naturally:
- `--clean` — force a clean `work/` re-extraction
- `--force` — bypass the output part skip check (always produce a new part)
- `--clean --force` — "start completely from scratch": re-extract sources and always write a new part

### Incremental builds

By default, `work/` is preserved across builds when the **build key** has not
changed. This allows plan authors to write lifecycle scripts that support
incremental compilation (e.g. `make` without `make clean` first). The
fetch/verify/extract steps are skipped entirely when `work/` is reused.

When the build key changes — because the version, sources, or lifecycle scripts
were modified — `work/` is automatically cleaned and sources are re-extracted.
To force a clean re-extraction without changing the plan, use `--clean`.

