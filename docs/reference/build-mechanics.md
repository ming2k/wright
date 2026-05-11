# Build Mechanics

This page explains what happens on disk when `wright build` executes: the build
directory layout, log files, source cache, and output parts.
Understanding these layers makes it easier to debug failures and reason about
when work is skipped or repeated.

## Two On-Disk Layers

`wright build` uses two different storage layers:

| Location | Purpose | Typical contents | Lifecycle |
|----------|---------|------------------|-----------|
| `build_dir` (default `/var/tmp/wright/workshop`) | Live working directory for a build | `work/`, `staging/`, `outputs/`, `logs/` | Scratch/workspace; may be deleted and recreated freely |
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
├── .wright-pipeline.json  # Stage state machine (hash-chain checkpoint records)
├── target/                # OverlayFS merge mount point (virtual root for the container)
├── .ovl_work/             # OverlayFS internal working directory
├── layers/                # Per-stage isolated directories
│   ├── 01-fetch/          # Hard-links to the global source cache
│   ├── 02-verify/         # (verification-only)
│   ├── 03-extract/        # Extracted source tree
│   ├── 04-prepare/        # Patched files
│   ├── 05-configure/      # ./configure output (Makefiles, config.h)
│   ├── 06-compile/        # .o files and binaries
│   ├── 07-check/          # (test-only)
│   └── 08-staging/        # make install output
├── staging/   # Convenience alias for final staging output
├── outputs/   # Sliced output directories (hard-linked from staging/)
│   └── default/  # Catch-all output
├── logs/      # Per-stage log files
└── .build_key # Build key marker (for work-tree reuse detection)

¹ When `version` is omitted from `plan.toml`, the directory uses `<name>-noversion`.
```

Each stage's writes are captured in its dedicated `layers/<NN>-<stage>/`
directory via OverlayFS.  The `target/` directory serves as the working
directory for the build — it presents a merged view of all completed layers
(lowerdir) with the current stage writing to its own upper layer.

`layers/` replaces the flat `work/` directory from previous versions.  When
the build key has not changed (same version, sources, and lifecycle scripts),
the layers from earlier stages (`fetch` through `extract`) are reused and only
stages whose inputs changed are re-executed.  Stage completion is tracked in
`.wright-pipeline.json` using a hash-chain fingerprint scheme — see
[Checkpoint Recovery](../explanation/checkpoint-recovery.md).

If multiple outputs are defined in `plan.toml` (split-parts), additional
output directories are created:

```
<build_dir>/<name>-<version>/¹
├── staging/         # Build script output (preserved for inspection)
├── outputs/
│   ├── default/     # Catch-all output (hard-linked from staging/)
│   └── <name>/      # Sub-part output (e.g. outputs/dev/)
│
¹ Uses `<name>-noversion` when `version` is omitted.
└── ...
```

During sub-part staging, the main part's output is mounted read-only at
`/main-part` (and available via `${MAIN_STAGING_DIR}`).


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
see `${STAGING_DIR}/usr/bin` in the output instead of the internal `/output/usr/bin`
path.

### Directory lifecycle rules

| Operation | `layers/` | `staging/` | `outputs/` | `logs/` | Checkpoints |
|-----------|:------:|:------:|:------:|:------:|:------:|
| Full build (key match) | **preserved** | recreated | recreated | recreated | **honored** (smart resume) |
| Full build (key mismatch) | recreated | recreated | recreated | recreated | cleared |
| `--stage=<s>` | preserved | recreated | recreated | recreated | ignored |
| `--force` build | reuse if key matches | recreated | recreated | recreated | **ignored** |
| `--clean` then build | deleted first | recreated | recreated | recreated | cleared |

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
file and symlink in `$STAGING_DIR` against the distribution's FHS whitelist before
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

The part is created from the output directories (`outputs/<name>/`) after the
output slicing phase and records the full part metadata (name, version,
dependencies, file list) for the installer.

#### Output slicing rules

When multi-output plans declare `[[output]]` entries:

1. Every file in `staging/` is evaluated against **all** non-catch-all outputs
   simultaneously.
2. A file matches an output when it hits any `include` glob and does **not**
   hit any `exclude` glob for that same output.
3. **Overlap is a build failure.** If a single file matches more than one
   output, slicing stops immediately with an error naming the file and the
   conflicting outputs. Use `exclude` patterns to carve out mutually exclusive
   sets.
4. Remaining files matching `[[discard]]` rules are ignored.
5. If a catch-all output exists (the one with no `include`), it keeps whatever
   remains in `staging/` via hard-links into `outputs/default/`.
6. Any file still unclaimed fails slicing and must be assigned or explicitly discarded.

`[[output]]` declaration order has no effect on slicing. The original `staging/`
directory is left intact for inspection after the build completes.

## Flag Quick Reference

| Flag | Source cache | Output part | `layers/` | `staging/` / `outputs/` / `logs/` | Stage checkpoints |
|------|:---:|:---:|:---:|:---:|:---:|
| (default) | reuse | skip if exists | reuse if key matches | recreated | **honored** |
| `--force` | reuse | overwrite | reuse if key matches | recreated | **ignored** |
| `--clean` | reuse | skip if exists | **deleted** | recreated | cleared |
| `--clean --force` | reuse | overwrite | **deleted** | recreated | cleared |
| `--stage=<s>` | reuse | skip | preserved | recreated | ignored |

`--clean` and `--force` address orthogonal concerns and compose naturally:
- `--clean` — force a clean `layers/` re-extraction; clears all stage checkpoints
- `--force` — bypass the output part skip check (always produce a new part) **and** re-run all lifecycle stages even when their checkpoints exist
- `--clean --force` — "start completely from scratch": re-extract sources, re-run all stages, and always write a new part

### Incremental builds

By default, `layers/` is preserved across builds when the **build key** has not
changed. The fetch and extract layers (01 through 03) are reused, and only
stages whose hash-chain fingerprint differs from the stored record are
re-executed. This means a repeated `wright build` with no changes completes
almost instantly — the smart resume algorithm in `.wright-pipeline.json` skips
all up-to-date stages automatically.

When the build key changes — because the version, sources, or lifecycle scripts
were modified — `layers/` is automatically cleaned and sources are
re-extracted. All checkpoint records in `.wright-pipeline.json` are cleared.

To force a clean re-extraction without changing the plan, use `--clean`.
To re-run all lifecycle stages while keeping `layers/` intact, use `--force`.
