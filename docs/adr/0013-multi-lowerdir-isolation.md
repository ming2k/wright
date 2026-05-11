# ADR-0013: Multi-lowerdir OverlayFS isolation

## Status

Accepted

Supersedes [ADR-0012](0012-overlayfs-per-task-upper.md).

## Context

ADR-0012 returned to OverlayFS-based strict isolation, using a pre-copied
sysroot (`/var/tmp/wright/sysroot/`) as the single shared read-only lower
layer.  This eliminated ETXTBSY by ensuring no host process could touch the
sysroot's inodes.

However, the sysroot approach carried significant costs:

- **Pre-copy time**: on first use (or after kernel/host-package updates),
  `ensure_global_sysroot()` must copy the entire host `/` tree, which is
  serial and slow (~seconds to minutes).
- **Disk usage**: the sysroot consumes 1–5 GB of duplicate files.
- **Staleness**: the cached sysroot drifts from the running host.  A new
  shared library installed by the host package manager is invisible inside
  builds until the sysroot is rebuilt.
- **Complexity**: `sysroot.rs` (426 lines) managed copy, validation,
  invalidation-timestamp tracking, and `chmod a-w` of every file.

The kernel's OverlayFS already enforces that the lower layer is read-only:
user-space writes go to the per-task upper layer via copy-up.  The lower
layer's inodes are never opened for writing through the overlayfs VFS path.
ADR-0012's `i_writecount` concern applies to direct host writes (outside
OverlayFS) to the same inode, but those are transient and the hardened
ETXTBUSY retry logic (10 attempts, randomised jitter, per-package logging)
handles them reliably.

## Decision

Use **multiple host system directories** (not a pre-copied sysroot) as
OverlayFS lower layers, with a **per-task writable upper layer**.

### Mechanism

1. **Multi-lowerdir construction**: canonicalize `/usr`, `/bin`, `/sbin`,
   `/lib`, `/lib64` on the host, deduplicate identical paths (e.g. on
   merged-/usr systems `/bin` → `/usr/bin` is already covered by `/usr`),
   and join the remaining unique directories with `:` as the `lowerdir=`
   option.

   ```
   lowerdir=/usr:/lib64:/lib   # typical on a merged-/usr host
   ```

2. **`/usr` hierarchy preservation**: after the OverlayFS mount, if the
   `newroot` does not contain a `/usr` directory (because the lowerdir
   was collapsed to a single parent on merged-/usr systems), bind-mount
   the host `/usr` at `newroot/usr` to restore the expected FHS hierarchy.
   This is necessary because `ld.so.cache` (bind-mounted from the host)
   references absolute paths such as `/usr/lib/libreadline.so.8`.

3. **Per-task upperdir/workdir**: each task gets empty directories under
   `{build_root}/.wright-isolation/{task_id}/upper/` and `work/`.

4. All other isolation mechanics (bind mounts for `/build`, `/output`,
   `/dev`, `/proc`, `/run`, `/tmp`, `/etc` files, `pivot_root`) are
   unchanged from ADR-0012.

### Why ETXTBSY is still eliminated

- **Kernel-enforced read-only lower layer**: OverlayFS does not permit
  writes through the overlay path to lower-layer files — any write triggers
  copy-up to the per-task upper layer.
- **Per-task upper layer**: copy-up'd files move to a per-task inode.
  Subsequent accesses use the private inode, eliminating contention.
- **Hardened retry logic**: for the edge case where a host process
  transiently holds a write reference to a lower-layer inode, the
  ETXTBUSY retry loop (10 attempts, randomised jitter, per-package log
  annotation) handles the collision.
- **No shared writeable inodes**: unlike the ADR-0010 bind-mount approach,
  there are no writable bind-mounts of shared paths.  `/build` and
  `/output` are task-private.  The `/usr` bind-mount (when present) is
  read-only.

## Consequences

### Positive

- **Zero pre-copy cost**: no sysroot to build or maintain.
- **Zero stale-sysroot problems**: builds always see the live host
  libraries and tools.
- **Zero additional disk usage**: no duplicate copy of the host filesystem.
- **Simpler code**: `sysroot.rs` and the global-sysroot pre-warm step are
  eliminated.
- **Kernel-enforced read-only lower layer**: identical ETXTBUSY protection
  as the sysroot approach for the common case.
- **Faster first build**: no upfront sysroot copy — isolation is ready
  immediately.

### Negative

- **ETXTBUSY edge case**: if a host process holds a write reference to a
  lower-layer inode at the exact instant a build task `execve()`s that
  file, the first attempt sees ETXTBUSY.  The retry logic absorbs this.
- **Kernel follows symlinks in lowerdir paths**: the `canonicalize` step
  is unavoidable for the mount option, so the deduplication logic must be
  preserved and the `/usr` hierarchy fix applied post-mount.

## Alternatives considered

| Approach | Reason for rejection |
|----------|----------------------|
| Pre-copied sysroot (ADR-0012) | Slow first build, disk overhead, staleness, complexity |
| `lowerdir=/` (full host root) | Exposes non-system paths; potential for dense `i_writecount` contention |
| Keep only system directories without `/usr` fix | `ld.so.cache` paths fail on merged-/usr hosts |
| No overlayfs (bind-mount, ADR-0010) | Proven to still exhibit ETXTBSY in the field |

## References

- `src/isolation/native.rs` — multi-lowerdir construction and `/usr` hierarchy fix
- `src/forge/pipeline.rs` — `ExecutorOptions` and pipeline stage execution
- `src/forge/executor.rs` — script execution and isolation dispatch
