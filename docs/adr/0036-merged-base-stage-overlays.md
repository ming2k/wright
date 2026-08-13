# ADR-0036: Merged-base stage layers with sandbox-mounted stage overlays

## Status

Accepted

## Context

Forge stage layering previously stacked OverlayFS mounts in the parent
mount namespace: stage N+1's overlay listed stage N's `layers/NN-stage`
directory (the previous stage's `upperdir`) inside its `lowerdir` stack.
Three kernel behaviors made that topology fragile:

1. **Exclusive upperdir protection.** The kernel takes an in-use lock on
   every overlay's `upperdir`/`workdir` and refuses to mount a new overlay
   whose `lowerdir` is an in-use `upperdir`/`workdir` of a still-living
   instance (`ovl_report_in_use`, `fs/overlayfs/super.c`).  When
   `CONFIG_OVERLAY_FS_INDEX=y` (so `index=on` is the default) the refusal is
   a hard `EBUSY`; the kernel's own comment frames it as papering over
   "mount leaks of container runtimes ... an old overlay mount is leaked and
   now its upperdir is attempted to be used as a lower layer in a new
   overlay mount" — exactly the stage-stacking pattern.
2. **Asynchronous superblock teardown.** The sandbox bind-mounted the
   parent-mounted stage overlay into its own mount namespace as `/build`,
   so the overlay superblock outlived the stage: it was released only after
   the last sandbox-namespace process was reaped (orphans killed on PID-1
   death, possibly D-state under load) plus deferred `mntput` work.  Under
   25-way parallel builds this exceeded 600 ms, while the mount retry
   budget was 300 ms.  On 2026-08-13 this repeatedly aborted
   `wright upgrade all` at abseil-cpp's configure→compile transition.
3. **Stale mounts after crashes.** A SIGKILLed run left stage overlays
   mounted in the parent mount table.  The lazy-detach recovery paths
   (`detach_stale_mounts`, `force_clean_dir`) kept builds limping forward
   but themselves extend superblock lifetimes, feeding failure mode (1).

The same race exists in comparable tooling — bubblewrap's native
`--overlay` reports "bwrap can exit while overlayfs upper directory is
still busy, preventing reuse in a subsequent command"
(containers/bubblewrap#672) — confirming this is an OverlayFS lifecycle
property, not an implementation bug in any one sandbox.

## Decision

### Merged base as the only lowerdir

Each stage's writes are still captured in `layers/NN-stage/` (the
overlay's `upperdir` while the stage runs).  After every stage, that delta
is merged into `base/` — a hard-link union of `source_dir` and all
completed stage layers.  The next stage's overlay mounts with
`lowerdir=base` as its *only* lower layer.

`base/` is never the `upperdir` or `workdir` of any overlay mount, so the
kernel's in-use upperdir protection can never fire on it, regardless of
how slowly a previous overlay dies.  The merge translates all three
OverlayFS delta encodings: whiteouts (char device `0:0`, or zero-length
files with `trusted.overlay.whiteout` / `user.overlay.whiteout` xattrs),
opaque directories (`*.overlay.opaque=y`), and fallback-mode tombstone
manifests (`.wright-layer-deletions`).

### Stage overlays mount inside the sandbox

The stage overlay is mounted by the isolation sandbox inside its own
mount namespace as `/build` (`IsolationConfig::stage_overlay`, helper
protocol v3).  The parent process never mounts stage overlays: mounts die
with the sandbox, crash/SIGKILL leaves zero stale mounts, and no
cross-namespace superblock references exist.

### Deterministic state reconciliation

`.base_manifest` records exactly what `base/` contains (source path plus
the canonical list of merged stages).  At forge start,
`LayerManager::reconcile_base` compares it against the checkpointed set of
completed stages: a match is a no-op; any mismatch (crash mid-merge,
checkpoint rewind, tampering) rebuilds `base/` from `source_dir` and the
surviving layers.

### Fresh inodes on retry

When a stage attempt is retried (ETXTBSY or mount `EBUSY`),
`LayerManager::freshen_upper_layer` renames the upper layer aside and
hard-links its contents into a brand-new directory.  Because the in-use
lock is taken on the upperdir/workdir *root inodes*, the retry mounts
fresh inodes that can never collide with the previous attempt's dying
overlay — without discarding the stage's work and without sleeping against
kernel teardown timing.

### Unprivileged cleanup

`remove_tree_force` restores owner `rwx` on directories before recursive
removal, because the kernel creates OverlayFS workdir internals with mode
000.  This makes the overlay path usable by unprivileged builds (which
mount inside a user namespace) rather than only as root.

## Alternatives

| Approach | Reason for rejection |
|----------|----------------------|
| `index=off` mount option | Downgrades the collision to a dmesg warning about undefined behavior.  Silences a kernel protection that catches genuine upperdir-sharing bugs; unnecessary once collisions are structurally impossible. |
| Larger mount retry budget | Kernel teardown latency is unbounded in principle (D-state orphan processes); treats the symptom. |
| btrfs/ZFS snapshot backend | Eliminates OverlayFS from stage layering, but ties a core build path to specific filesystems.  Remains a possible future opt-in backend. |
| fuse-overlayfs | Userspace overlay; IO performance cost; solves rootless mounting, not the in-use lifecycle race. |
| bubblewrap / OCI runtimes | Hit the same kernel race (containers/bubblewrap#672); the kernel check exists because of container-runtime mount leaks.  Adds external dependencies without addressing the cause. |

## Consequences

### Positive

- Stage-transition `EBUSY` is impossible by construction: the lowerdir is
  never in-use, and retry attempts mount fresh upper/work inodes.
- The parent mount table is untouched by builds; crash cleanup code
  (`detach_stale_mounts`) only serves pre-existing build roots created by
  older versions.
- Crash recovery is deterministic: `base/` always converges to
  `source_dir` + checkpointed layers.
- Unprivileged builds use the real overlay path instead of the hard-link
  fallback wherever user namespaces are available.

### Negative

- Each stage pays an O(delta) hard-link merge; resume/rewind pays an
  O(tree) rebuild.  Both are negligible next to compilation.
- The merge must mirror OverlayFS delta encodings (whiteouts, opaque
  directories); a kernel that introduces new encodings must be matched in
  `foundry/layers.rs`.
- The isolation helper wire protocol moves to version 3; stale helper
  binaries fail closed.
- Fallback (`isolation = "none"`) stages still share inodes with `base/`
  via hard-links; tools that rewrite files in place can corrupt the layer
  bookkeeping there.  This caveat predates this ADR and is unchanged.

## References

- `crates/wright-engine/src/foundry/layers.rs` — merged base, reconcile,
  merge, freshen, `remove_tree_force`
- `crates/wright-engine/src/isolation/native/run.rs` — in-sandbox stage
  overlay mount
- `crates/wright-engine/src/foundry/forge/mod.rs` — stage loop wiring
- [ADR-0013](0013-multi-lowerdir-isolation.md) — sandbox root overlay
  (unchanged by this ADR)
- [OverlayFS Layers](../explanation/overlayfs-layers.md) — concept-level
  walkthrough
- [containers/bubblewrap#672](https://github.com/containers/bubblewrap/issues/672) —
  the same race in bubblewrap
- Linux `fs/overlayfs/super.c` — `ovl_report_in_use` and the in-use lock
