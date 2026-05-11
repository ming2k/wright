# ADR-0010: Pre-copied read-only sysroot instead of OverlayFS for strict isolation

## Status

Superseded by [ADR-0012](0012-overlayfs-per-task-upper.md)

## Context

Wright's strict isolation used Linux OverlayFS with `lowerdir=/` as the root filesystem for each sandboxed build task.  The upperdir and workdir were per-task scratch directories under `/var/tmp/wright/workshop/<task>/.wright-isolation/`.

This design caused a reproducible `ETXTBSY` ("Text file busy") failure when multiple tasks executed shebang scripts (`./configure`, `make` sub-processes, etc.) in parallel.  The error manifested as:

```
./configure: /bin/sh: bad interpreter: Text file busy
```

### Root cause

OverlayFS `lowerdir=/` shares the **host's live inode cache** across all parallel mount instances.  When task A triggers a copy-up (e.g. opening a file for write, creating a whiteout, or even a dentry revalidation), the kernel briefly holds a write-reference to the lower-layer inode.  Task B's concurrent `execve("/bin/sh")` sees that reference and returns `ETXTBSY`.

This is not a bug in wright's plan files, in the upstream packages, or in the Linux kernel.  It is a fundamental property of OverlayFS when `lowerdir` is a **mutable, live filesystem** that other processes are actively using.

### Why workarounds were rejected

| Approach | Reason for rejection |
|----------|----------------------|
| Serialise script execution (global lock around `exec`) | Destroys parallel build performance; 14-core machine becomes effectively single-threaded during configure stages |
| Per-task private lowerdir (bind-mount host root, then OverlayFS on that) | Still shares the host's underlying ext4/btrfs inode cache; reduces but does not eliminate the race |
| SquashFS system image | Would work, but requires `mksquashfs` at runtime and adds a block-device/loop-mount layer.  Overkill when a simpler solution exists. |
| Btrfs reflink snapshots | Requires btrfs; wright must remain filesystem-agnostic |

## Decision

Replace OverlayFS with a **pre-copied, read-only sysroot** that is created once and reused by all strict-isolation tasks.

### Mechanism

1. **First call to `ensure_global_sysroot()`** (triggered at the start of `wright apply`):
   - Copy `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64` and essential `/etc` files into `/var/tmp/wright/sysroot/`
   - Use `cp -a` (or pure-Rust equivalent when `cp` is BusyBox) with metadata preservation
   - Recursively `chmod -R a-w` the entire tree
   - Write an mtime-based stamp file for cache invalidation

2. **Per-task isolation setup**:
   - Mount the sysroot directory (recursive bind) as the new root
   - Remount it read-only
   - Mount tmpfs for `/tmp`, `/run`
   - Bind-mount task-specific `work/` and `output/` directories as read-write
   - `pivot_root` and execute

3. **Concurrency control**:
   - An `flock(LOCK_EX)` on `sysroot.lock` prevents multiple parallel tasks from racing on the initial copy
   - After creation, all tasks read the same immutable files without any kernel-level locking

### Why this eliminates ETXTBUSY

- `/var/tmp/wright/sysroot/bin/sh` is a **copy**, not the host's `/bin/sh`
- No host process knows this path, therefore **no host process can open it for writing**
- `chmod a-w` makes the permission check fail early, so the kernel never needs to arbitrate write access
- Multiple tasks executing the same copied inode is pure read-sharing, which Linux handles without `ETXTBSY`

## Consequences

### Positive

- **ETXTBSY is eliminated**, not merely reduced
- **No filesystem dependency**: works on ext4, btrfs, xfs, tmpfs, or any POSIX filesystem
- **No external tools required**: the copy is implemented in pure Rust; no `mksquashfs`, `cp`, or `chmod` binaries needed
- **Performance**: after the one-time copy (typically 5–30 s on first run), task startup is a single bind-mount — comparable to or faster than OverlayFS setup
- **Cache invalidation**: the sysroot is automatically rebuilt when the host system directories have newer mtimes

### Negative

- **Disk space**: a full copy of `/usr` + `/bin` + `/lib` consumes roughly 200 MB–2 GB depending on the host.  This is comparable to Docker image layer overhead and acceptable for build servers.
- **First-run latency**: the initial copy is not instantaneous.  This is mitigated by running `ensure_global_sysroot()` eagerly at the start of `apply`, before any build tasks begin.
- **No copy-on-write**: unlike OverlayFS or btrfs reflink, modifying the sysroot would require a full copy.  This is irrelevant because the sysroot is **never modified** after creation.

## Alternatives considered

See "Why workarounds were rejected" in Context above.

## References

- `src/isolation/sysroot.rs` — `SysrootManager` implementation
- `src/isolation/native.rs` — namespace isolation with sysroot bind-mount
- `src/forge/executor.rs` — hook that sets `base_root` to the sysroot path
- Linux kernel documentation: `Documentation/filesystems/overlayfs.rst` ( OverlayFS lowerdir sharing behaviour )
