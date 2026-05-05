# ADR-0012: OverlayFS with per-task writable upper layers

## Status

Accepted

Supersedes [ADR-0010](0010-pre-copied-sysroot-isolation.md).

## Context

ADR-0010 replaced OverlayFS with a pre-copied read-only sysroot that was
bind-mounted into each task's isolation environment.  The rationale was that
OverlayFS with `lowerdir=/` caused ETXTBSY races when multiple tasks shared
the host's live inode cache.

However, the bind-mount approach did not fully eliminate ETXTBSY.  When
multiple tasks bind-mounted the same sysroot inodes, concurrent `execve()`
could still contend: `deny_write_access()` serialises on the inode-level
`i_writecount`, and a write-reference can leak across bind-mount instances
on certain kernel/filesystem configurations.

## Decision

Return to OverlayFS, but with a **shared read-only lower layer** (the
pre-copied sysroot) and a **per-task writable upper layer**.

### Mechanism

1. **Shared lowerdir**: The pre-copied sysroot (`/var/tmp/wright/sysroot/`),
   built once by `ensure_global_sysroot()`, serves as the read-only lower
   layer — as in ADR-0010.

2. **Per-task upperdir/workdir**: Each task gets its own empty directories
   under `{build_root}/.wright-isolation/{task_id}/upper/` and `work/`.

3. **OverlayFS mount**:
   ```
   mount -t overlay overlay \
     -o lowerdir={sysroot},upperdir={upper},workdir={work} \
     {newroot}
   ```

4. All other isolation mechanics (mount namespace, bind mounts for /build,
   /output, /dev, /proc, /run, /tmp, /etc files, pivot_root) are unchanged.

### Why this eliminates ETXTBSY

The original OverlayFS bug (`lowerdir=/`) was not an inherent flaw of
overlayfs — it was a flaw in using the **host's live root** as the shared
lower layer.  Host processes continuously open, write, and close files
under `/`, incrementing and decrementing `i_writecount` at high frequency.
A parallel task's `execve()` could then observe `i_writecount > 0` and
return ETXTBSY.

With a pre-copied, **immutable** sysroot as lowerdir:

- No host process knows or touches the sysroot path.
- `chmod a-w` on every file prevents accidental opens for writing.
- `i_writecount` on every lower inode stays permanently at 0.
- `deny_write_access()` always succeeds.

Overlayfs itself does not increment `i_writecount` on lower inodes during
normal operation (mount setup, lookup, read, exec) — only the kernel's
copy-up path does, and copy-up only triggers when a file is opened for
**writing**.  All build output goes to `/build` and `/output` (per-task
bind mounts), so operating-system paths in the overlay are never opened for
writing.  Copy-up is a safety net, not the primary mechanism.

The presence of a per-task upper layer additionally provides:

- **Write isolation**: any unexpected writes to system paths are captured
  in the per-task upper, not shared.
- **Copy-up as fail-safe**: if a build does write to a system path,
  overlayfs lifts the file to the private upper layer, and subsequent
  references go through a per-task inode.

### Bind-mount vs overlayfs inode sharing

In the bind-mount design (ADR-0010), the shared inode was the ONLY path to
a file — every access went through that inode, and any inode-level
contention was permanent.  With overlayfs, when a file is copied up to the
upper layer, its data is served from a per-task inode, permanently
eliminating contention for that path.

## Consequences

### Positive

- **ETXTBSY eliminated**: immutable lower layer means `i_writecount` stays
  0; copy-up provides an escape hatch for any edge case.
- **No filesystem dependency**: works on any filesystem supported by Linux
  overlayfs (ext4, xfs, btrfs, tmpfs, etc.).
- **No reflink/CoW dependency**: does not require btrfs or XFS reflink
  support.
- **Per-task write isolation**: any writes to system paths during the build
  are captured in the per-task upper layer and do not affect the sysroot
  or other tasks.
- **No additional disk overhead**: upper/work directories start empty.
- **Unchanged sysroot caching**: the existing `ensure_global_sysroot()`
  mechanism is reused as-is.

### Negative

- **OverlayFS dependency**: requires kernel with overlayfs support.  This
  is ubiquitous on modern Linux distributions.
- **upper/lower/work must share a filesystem**: the sysroot is placed in
  the default build root (`/var/tmp/wright/`), and per-task scratch
  directories are placed under the task's build root — these are typically
  the same filesystem.

## Alternatives considered

| Approach | Reason for rejection |
|----------|----------------------|
| Per-build reflink copy (btrfs FICLONE) | Requires Copy-on-Write filesystem; not filesystem-agnostic |
| Serialise script execution | Destroys parallel build performance |
| SquashFS system image | Requires `mksquashfs` and block-device/loop-mount layer |
| Keep bind-mount approach (ADR-0010) | Proven to still exhibit ETXTBSY in the field |

## References

- `src/isolation/native.rs` — overlayfs mount and per-task upper/work setup
- `src/isolation/sysroot.rs` — `SysrootManager` for shared lower layer
- `src/builder/executor.rs` — hook that sets `IsolationConfig.base_root` to sysroot path
