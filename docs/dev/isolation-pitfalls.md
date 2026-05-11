# Isolation Race Handling

Known race conditions and filesystem traps that every contributor should
understand before touching `src/isolation/`, lifecycle execution, or build
output slicing.

For the user-facing architecture model, see
[Isolation Model](../explanation/isolation-model.md).  The accepted design
record is [ADR-0013](../adr/0013-multi-lowerdir-isolation.md).

## btrfs subvolume EXDEV (hard-link across subvolumes)

### Symptom

```
build error: failed to hard-link /var/tmp/wright/workshop/<name>-<version>/staging/.../file
to /var/tmp/wright/workshop/<name>-<version>/outputs/default/.../file:
Invalid cross-device link (os error 18)
```

### Root cause

btrfs subvolumes share the same filesystem mount point but have independent
inode spaces.  Hard links cannot cross subvolume boundaries — the kernel
returns `EXDEV` even though `st_dev` may appear to match.

This is triggered when:

1. The host uses separate btrfs subvolumes for `/` and `/var`.
2. Wright's build staging directory lives under `/var/tmp/wright/...`
   (`/var` subvolume).
3. Strict isolation uses overlayfs whose upper/work layers are placed
   under `{build_root}/.wright-isolation/`, which also lives on `/var`.
4. However, during mount setup the overlayfs `upperdir` directory is
   created **before** the bind-mount for `/output` is applied.  If a
   build process later creates a directory under `/output/...` and the
   kernel's VFS path walk caches a dentry from the overlayfs upper layer
   rather than traversing through the bind-mount, that directory inode
   may land on the root subvolume instead of `/var`.
5. When output slicing hard-links files from staging into `outputs/`,
   per-file inodes on different subvolumes cause `EXDEV`.

### Fix

The `link_or_copy()` helper in `src/forge/mod.rs` catches `EXDEV` from
`hard_link()` and transparently falls back to `fs::copy`.  This is
functionally equivalent — the only cost is extra disk space for the
duplicated data of the affected files.

### Prevention

- When adding new hard-link sites in build output handling, always use
  `link_or_copy()`, never `tokio::fs::hard_link()` directly.
- When changing overlayfs upper/work directory placement, verify that
  the upper layer is on the same filesystem as the staging directory.

## ETXTBSY (Text file busy) with shebang scripts

### Symptom

```
${WORKDIR}/.wright_script.sh: ./configure: /bin/sh: bad interpreter: Text file busy
```

Exit code 126.

### Root cause

When multiple parallel build tasks exec the same interpreter binary (e.g.
`/bin/sh`) through shebang resolution, the kernel's `deny_write_access()`
can fail on the shared inode.  This is a per-inode race, not a per-path race.

Strict isolation uses OverlayFS with host system directories as read-only lower
layers and a per-task writable upper layer.  OverlayFS prevents writes through
the overlay path from mutating lower-layer files; those writes copy up into the
task-private upper layer instead.

That removes the shared writable-inode failure mode from the normal build path,
but an edge case remains: a host process can briefly hold a direct write
reference to a lower-layer inode at the exact moment a build task tries to
execute it.  Certain kernel/filesystem combinations can also report ETXTBSY
during tight parallel exec windows.

### Fix layered defence

1. **Multi-lowerdir OverlayFS with per-task upper** (`src/isolation/native.rs`):
   host system directories are mounted as lower layers and each task gets a
   private upper/work pair.  If a file is opened for writing through the
   overlay path, copy-up moves it to the task-private upper layer.

2. **execvp retry loop** (`src/isolation/native.rs`): 8 retries with
   exponential backoff for the top-level `execvp(command)` call.

3. **Stage-level retry** (`src/forge/pipeline.rs`): when a pipeline stage
   exits with code 126 and its output contains "Text file busy", the stage
   is retried up to 10 times with capped exponential backoff (200ms-1000ms
   base) and randomized jitter on each delay.  This catches ETXTBSY from
   shebang-level execs that happen inside the shell (e.g. `./configure`
   → kernel resolves `#!/bin/sh` → `/bin/sh` busy) — a path the lower-level
   `execvp` retry loop never sees, because that retry only fires for the
   top-level command (`/bin/bash`).

   The jitter is critical: with N parallel tasks all hitting the same
   shared-inode race, a deterministic backoff causes every retrier to
   wake at the same instant and re-collide.  Jittered delays spread the
   retries across the recovery window so the contention drains.

### Prevention

- When adding new exec sites inside isolation, consider whether the
  exec'd binary could be a shebang script that chains to a shared
  interpreter.
- Do not remove or weaken the stage-level ETXTBSY retry.
- Preserve jitter in retry delays.  Deterministic backoff can synchronize
  parallel retriers and recreate the collision.

## Bind-mount ordering vs overlayfs directory shadowing

### Symptom

Files or directories created under `/output/...` inside isolation appear
on an unexpected filesystem (different `st_dev` from the bind-mount target).

### Mechanism

1. The overlayfs is mounted at `newroot`.
2. `mkdir -p newroot/output` creates `output/` in the overlayfs **upper**
   layer.
3. `mount --bind staging_dir newroot/output` binds the host staging
   directory over `newroot/output`.

Under normal operation the bind-mount correctly shadows the overlayfs.

However, if overlayfs dentry cache entries for `/output/.../subdir` are
populated **before** the bind-mount takes effect (e.g. by `create_dir_all`
during mount setup, or by a previous task reusing the same scratch path
without cleanup), a later `mkdir -p /output/.../subdir` from the build
script may follow the cached dentry into the upper layer rather than
tunnelling through the bind-mount.

### Fixes in place

- The `bind` helper in `native.rs` unconditionally removes the destination
  if it is a symlink, then creates it fresh before mounting.
- Each task uses a unique `task_id` in its scratch path, preventing
  accidental reuse of upper-layer dentries from previous tasks.
- Output slicing uses `link_or_copy()` as a last-resort safety net.

### Prevention

- Do not change the order of overlayfs mount → bind-mount setup.
- Do not reuse isolation scratch directories across tasks without
  cleaning the upper layer.
- When modifying mount setup, verify that files written under `/output`
  and `/build` inside isolation land on the expected host directories.
