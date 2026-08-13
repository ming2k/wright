# Isolation Race Handling

Known race conditions and filesystem traps that every contributor should
understand before touching `src/isolation/`, pipeline execution, or build
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

The `link_or_copy()` helper in `src/foundry/mold.rs` catches `EXDEV` from
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

1. **Multi-lowerdir OverlayFS with per-task upper** (`src/isolation/native/run.rs`):
   host system directories are mounted as lower layers and each task gets a
   private upper/work pair.  If a file is opened for writing through the
   overlay path, copy-up moves it to the task-private upper layer.

2. **execvp retry loop** (`src/isolation/native/run.rs`): 8 retries with
   exponential backoff for the top-level `execvp(command)` call.

3. **Stage-level retry** (`src/foundry/forge/execute.rs`): when a pipeline stage
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

## EBUSY on stage transition (in-use upperdir as lowerdir) — eliminated by merged base

### Symptom (pre-ADR-0036)

```text
error: forge abseil-cpp: failed to mount overlay at
/var/tmp/wright/workshop/abseil-cpp-20260526.0/stages/03-compile: EBUSY: Device or resource busy
```

Happened at stage transitions under heavy parallel load, and reproduced
across retries whenever the previous run had left stale mounts behind.

### Root cause

The old `LayerManager` mounted each stage's overlay in the **parent**
mount namespace and stacked per-stage layers: stage N+1's `lowerdir`
included stage N's `layers/NN` directory — the previous stage's
`upperdir`.  The kernel takes an exclusive in-use lock on every overlay's
`upperdir`/`workdir` and refuses (with `index=on`, the default under
`CONFIG_OVERLAY_FS_INDEX=y`) to mount an overlay whose `lowerdir` is an
in-use `upperdir` of a still-living instance.

Because the sandbox also bind-mounted the same overlay into its own mount
namespace, the superblock stayed alive after the stage exited — released
only when the last sandbox-namespace process was reaped (orphans killed
asynchronously on PID-1 death, possibly D-state under load) plus deferred
kernel `mntput`.  Observed lag exceeded 600 ms under a 25-package batch;
the mount retry budget was 300 ms.

### Fix (architecture, not retry)

See [ADR-0036](../adr/0036-merged-base-stage-overlays.md):

1. Each stage's delta is merged into `base/` after the stage; the next
   stage's overlay uses `lowerdir=base` only.  `base/` is never an
   `upperdir`/`workdir`, so the in-use lock can never fire on it.
2. Stage overlays are mounted **inside the sandbox's mount namespace**.
   They die with the sandbox, so crashes leave no stale mounts and no
   cross-namespace superblock references.
3. Stage retries call `LayerManager::freshen_upper_layer`, which renames
   the layer aside and hard-links it back into a brand-new directory: the
   in-use lock binds the upperdir root *inode*, so the retry's fresh inode
   cannot collide with the previous attempt's dying overlay.

### Prevention

- Never introduce a mount whose `lowerdir` is, or contains, another
  overlay's current `upperdir`/`workdir`.  If a design seems to need it,
  merge into a neutral directory first.
- Never mount stage overlays from the parent process; pass
  `IsolationConfig::stage_overlay` and let the sandbox mount them.
- Do not reintroduce time-based mount retries as the primary defence
  against in-use collisions; fresh inodes are the deterministic fix.

## EBUSY on cleanup (stale overlay mount from prior run) — legacy build roots only

### Symptom

```text
build error: failed to clean forge directory /var/tmp/wright/workshop/bison-3.8.2:
Device or resource busy (os error 16)
```

Happens at the start of a fresh forge, when Wright tries to wipe the build
root before populating it.

### Root cause

Current Wright mounts stage overlays inside the sandbox namespace, so a
dead build leaves no mounts behind.  This failure now only comes from
build roots created by **pre-ADR-0036 Wright versions**, which mounted
stage overlays in the parent namespace: an abnormal exit (SIGKILL, power
loss, panic, OOM) left the overlay mount active in the kernel mount table,
and the next run's `remove_dir_all` walked into the still-mounted
directory and got `EBUSY` — you cannot `rmdir` a directory that is itself
a mount point.

### Fix in place

`force_clean_dir` (`src/foundry/layers.rs`) wraps every `Foundry::clean` removal:

1. Attempt `remove_dir_all`. Return on success or `NotFound`.
2. On `EBUSY`, call `detach_mounts_under(path)`:
   - Parse `/proc/self/mounts` line-by-line.
   - Filter to mounts whose target is `path` or a descendant.
   - Sort deepest-first so parents become free as we unmount.
   - `umount2(target, MNT_DETACH)` each one. Lazy detach succeeds even if
     a process still has the mount open.
3. Sleep `100ms · 2^attempt` and retry, up to 3 attempts total.
4. Return the original `EBUSY` if all retries fail (something other than
   stale mounts is holding the directory).

The whole helper is `tokio::task::spawn_blocking`'d because `remove_dir_all`
and `umount2` are synchronous syscalls.

### Why /proc/self/mounts instead of tracking what we mounted

The point of this recovery path is to clean up mounts the **previous**
process created. The current process has no in-memory record of those
mounts, so we have to ask the kernel via `/proc/self/mounts`. Tracking
"what we mounted" only helps when the same process unmounts; the failure
mode here is exactly that the original process is gone.

### Prevention

- Do not bypass `Forger::clean` with a direct `remove_dir_all` of build
  artifacts. The helper is the right entry point for forge-directory
  cleanup.
- When adding new mounts under the forge directory, ensure they are
  unmounted on every error path (RAII via `Drop` is the cleanest pattern).
- The recovery is silent at INFO level — operators do not see a warning
  when self-healing succeeds. If you find a need to debug, set
  `WRIGHT_LOG=debug` and look for `event = "clean.ebusy"` and
  `event = "clean.umount"` records.

## EACCES on cleanup (kernel-created mode-000 overlay workdirs)

### Symptom

```text
forge error: failed to clear overlay work dir
/var/tmp/wright/workshop/<name>-<version>/.ovl_work/03-compile:
Permission denied (os error 13)
```

### Root cause

The kernel creates OverlayFS workdir internals (`work/`) with mode 000.
Root walks straight through, but an unprivileged owner cannot traverse the
directory, so `remove_dir_all` fails with `EACCES`.  Before ADR-0036 this
path was unreachable for unprivileged builds (overlay mounts required
root); with stage overlays mounted inside the user-namespace sandbox,
unprivileged builds use the overlay path too and hit this on resume and
retry cleanup.

### Fix in place

`remove_tree_force` (`src/foundry/layers.rs`) restores owner `rwx` on every
directory in a tree before removing it.  All forge-side recursive
removals (layer dirs, overlay workdirs, `base/`, `force_clean_dir`) route
through it.

### Prevention

- Never call bare `remove_dir_all` on forge-owned trees; route through
  `remove_tree_force` or `force_clean_dir`.

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
