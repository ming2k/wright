# Build Mechanics

This page explains what happens on disk when `wright build` executes: the build
directory layout, log files, source cache, and output parts.
Understanding these layers makes it easier to debug failures and reason about
when work is skipped or repeated.

## Two On-Disk Layers

`wright build` uses two different storage layers:

| Location | Purpose | Typical contents | Lifecycle |
|----------|---------|------------------|-----------|
| `forge_dir` (default `/var/tmp/wright/workshop`) | Live working directory for a build | `work/`, `staging/`, `outputs/`, `logs/` | Scratch/workspace; may be deleted and recreated freely |
| `source_dir` (default `/var/lib/wright/sources`) | Reusable source input cache | Downloaded tarballs, zip files, git source snapshots | Persistent cache across builds |

### How the two layers relate

- `source/` (with the `.charge_prepared` marker beside it) holds the previously
  materialized source tree, and decides whether Wright may reuse it.
- `source cache` decides whether Wright must re-download or re-copy source inputs.

Execution order:

1. Resolve every source to its content address and hash the set into a
   `SourceDigest` (ADR-0054).
2. Reuse `source/` only when `.charge_prepared` records that same digest, the
   tree is still present, and no source is volatile.
3. Otherwise fetch/extract from `source cache` and rewrite the marker.

## Build Directory Layout

Each part gets its own working directory under `forge_dir`
(default `/var/tmp/wright/workshop`):

```
<forge_dir>/<name>-<version>/¹
├── .wright-checkpoint.json  # Stage state machine (hash-chain checkpoint records)
├── source/                # Immutable extracted source tree (Charge output)
├── base/                  # Merged base: reflink/hard-link union of source +
│                          # completed layers (source of the next stage's target/)
├── .base_manifest         # Records exactly what base/ contains (rebuild trigger)
├── target/                # Real directory: the stage's working tree (/build)
├── layers/                # Per-stage harvested delta directories
│   ├── 01-prepare/        # Patched files
│   ├── 02-configure/      # ./configure output (Makefiles, config.h)
│   ├── 03-compile/        # .o files and binaries
│   ├── 04-check/          # (test-only)
│   └── 05-staging/        # Files installed into the build tree
├── staging/   # Convenience alias for final staging output
├── outputs/   # Sliced output directories (hard-linked from staging/)
│   └── default/  # Catch-all output
├── logs/      # Per-stage log files
└── .charge_prepared  # Source content address this tree was materialized from

¹ When `version` is omitted from `plan.toml`, the directory uses `<name>-noversion`.
```

Before a stage runs, `target/` is populated from `base/` (reflinks where
the filesystem supports them, hard links otherwise) and the sandbox
bind-mounts it as `/build` — a real directory tree with ordinary
filesystem semantics, not an OverlayFS mount.  After the stage exits, its
delta is harvested into `layers/<NN>-<stage>/` (deletions recorded as
tombstones) and merged into `base/`, so the next stage always starts from
the full accumulated tree.  Nothing is mounted per stage, so crashed
builds leave no stale mounts behind.  See
[Stage Layers, the Merged Base, and OverlayFS](../explanation/overlayfs-layers.md).

`layers/` replaces the flat `work/` directory from previous versions.  When
the source content address and the pipeline scripts are unchanged, the layers
from earlier stages are reused and only
stages whose inputs changed are re-executed.  Stage completion is tracked in
`.wright-checkpoint.json` using a hash-chain fingerprint scheme — see
[Checkpoint Recovery](../explanation/checkpoint-recovery.md).

If multiple outputs are defined in `plan.toml` (split-parts), additional
output directories are created:

```
<forge_dir>/<name>-<version>/¹
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

Every pipeline stage writes a log file under `logs/`:

```
<forge_dir>/<name>-<version>/logs/
├── configure.log
├── compile.log
├── staging.log
└── ...
```

Each file contains:

```
=== Stage: compile ===
=== Exit code: 0 ===
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

### Permissions and Diagnostic Accessibility

In accordance with ADR-0050 and ADR-0053, Wright enforces deterministic, umask-immune permissions across the forge, package sealing, and deployment pipelines:
- **Build Root and Log Directories** (`<forge_dir>/<name>-<version>/`, `logs/`, `staging/`, `outputs/`): created with mode `0755` (`drwxr-xr-x`).
- **Stage Logs** (`logs/*.log`, `slice-errors.log`): created with mode `0644` (`-rw-r--r--`).
- **Isolation Scratch** (`.wright-isolation/`): created with mode `0700` (`drwx------`) to strictly isolate kernel mounts and OverlayFS work directories.
- **Triple-Defense Payload Immunity (ADR-0053)**:
  1. *Subprocess Execution*: Sandbox child processes, direct runners, and deploy hooks reset process umask to `0022` prior to command execution (`[INV-PERM-04]`).
  2. *Archive Sealing*: Package archives (`.wright.tar.zst`) canonicalize directories to `0755` (preserving sticky/setgid bits), executable binaries to `0755`, and regular data files to `0644` (`[INV-PERM-05]`).
  3. *Deployment Materialization*: Directory hierarchies created during rootfs deployment explicitly enforce `0755`, self-healing any pre-existing restricted directories and preventing ambient caller umasks (e.g. `sudo`'s `0077`) from contaminating host paths (`[INV-PERM-06]`).

Even when builds and installations are triggered under privileged execution (`sudo wright install` or `sudo wright build`) where the host environment enforces a restrictive umask (`0077`), logs, staging trees, sealed archives, and target rootfs directories remain readable and traversable by unprivileged users (`[INV-PERM-07]`).

### On failure

When a stage exits non-zero, the last 40 lines of stderr (or stdout if stderr
is empty) are printed to the terminal directly. The full output is always in
the log file. Logs from the failed run are **preserved** — they are only
overwritten on the next build attempt.

**Golden Standard:** Wright automatically maps internal sandbox paths back to
their corresponding variables in error messages. If a script fails, you will
see `${STAGING_DIR}/usr/bin` in the output instead of the internal `/output/usr/bin`
path.

### Directory pipeline rules

| Operation | `layers/` | `staging/` | `outputs/` | `logs/` | Checkpoints |
|-----------|:------:|:------:|:------:|:------:|:------:|
| Full build (key match) | **preserved** | recreated | recreated | recreated | **honored** (smart resume) |
| Full build (key mismatch) | recreated | recreated | recreated | recreated | cleared |
| `--stage=<s>` | preserved | recreated | recreated | recreated | ignored |
| `--force` build | reuse if key matches | recreated | recreated | recreated | **ignored** |
| `--fresh` then build | deleted first | recreated | recreated | recreated | cleared |
| `--no-cache` build | **purged first** | recreated | recreated | recreated | cleared |

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
└── linux-6.9.8-1a2b3c4d.tar.zst   # git source snapshot: <repo>-<ref>-<url-hash>
```

The filename is prefixed with the part name to avoid collisions between
plans that use similarly-named dependency parts (e.g. two parts both
fetching `v1.0.tar.gz` from different projects). The prefix is a cache-layer
detail only: non-archive sources are placed in the work directory under
their original basename (ADR-0024).

Before extraction, each source is verified against its `sha256` checksum from
`plan.toml`. If the cached file fails verification, it is deleted and
re-downloaded. Local path sources use `"SKIP"` as their checksum and bypass
verification.

The source cache is only consulted when Wright needs to materialize `work/`
again. If `work/` is reusable, it is not used in that run.

`--no-cache` transactionally purges this directory and the CAS `store_dir`
before building. Verified pinned `http` entries — files whose bytes hash to the
declared `sha256` for the selected build set — are kept and reused; git
named-ref snapshots, `SKIP` entries, and local cache copies are discarded. That
is the only way to pick up a git ref that moved under the same name: a snapshot
is an explicit pin, so its content address does not change when the ref does
(ADR-0054, ADR-0057). The purge is staged and journaled, so a crash leaves the
cache either untouched or fully purged. `parts_dir` archives are never deleted;
they are the local inventory, not a cache.

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

There is no skip condition keyed on a part archive already existing in
`parts_dir`: each build re-runs charge/forge and seal writes a fresh archive.
Reuse is decided earlier and from content, by the charge marker, the forge
checkpoints, and the CAS fingerprint. Use `--force` or `--no-cache` to deny all
of them; neither deletes the previous archive, which remains the rollback copy
until seal overwrites it.

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
| (default) | reuse | rewrite | reuse if key matches | recreated | **honored** |
| `--force` | reuse | rewrite | reuse if key matches | recreated | **ignored** |
| `--fresh` | reuse | rewrite | **deleted** | recreated | cleared |
| `--no-cache` | **purged (verified pinned kept)** | rewrite | **deleted** | recreated | cleared |
| `--fresh --force` | reuse | rewrite | **deleted** | recreated | cleared |
| `--stage=<s>` | reuse | rewrite | preserved | recreated | ignored |

`--fresh` and `--force` address orthogonal concerns and compose naturally:
- `--fresh` — force a clean `layers/` re-extraction; clears all stage checkpoints
- `--force` — force seal to re-slice `outputs/` from `staging/` (the archive itself is always rewritten) **and** re-run all pipeline stages even when their checkpoints exist
- `--fresh --force` — "start completely from scratch": re-extract sources, re-run all stages, and always write a new part
- `--no-cache` — a strictly stronger, explicit mode (ADR-0057): transactionally purge the unverifiable source cache and the CAS store (verified pinned inputs are kept), refetch every unproven source, and deny every reuse decision. `--force` and `--fresh` keep their meanings; `--no-cache` does not redefine them.

### Incremental builds

By default, `layers/` is preserved across builds when the **build key** has not
changed. The extracted source tree and completed stage layers are reused, and only
stages whose hash-chain fingerprint differs from the stored record are
re-executed. This means a repeated `wright build` with no changes completes
almost instantly — the smart resume algorithm in `.wright-checkpoint.json` skips
all up-to-date stages automatically.

The build key is derived from the plan's metadata, its pipeline scripts, and the
**content address of its resolved sources** (ADR-0054). Editing a `local` source
file changes that address, so the source tree is re-materialized and every stage
is re-run — there is no path by which an edited file can be reported as up to
date. A plan whose sources cannot be content-addressed (`git_metadata = true`, or
an `http` `SKIP` source not yet cached) is *volatile*: it gets no reuse at all,
and every build re-materializes and re-runs from scratch.

When the build key changes — because the version, sources, or pipeline scripts
were modified — `layers/` is automatically cleaned and sources are
re-extracted. All checkpoint records in `.wright-checkpoint.json` are cleared.

To force a clean re-extraction without changing the plan, use `--fresh`.
To re-run all pipeline stages while keeping `layers/` intact, use `--force`.
To also discard the unverifiable source cache and the CAS — the only way to see
a named git ref move — use `--no-cache` (ADR-0057). It stages and journals the
purge, keeps verified pinned inputs, and rebuilds from freshly fetched sources;
`parts_dir` archives are left in place.
