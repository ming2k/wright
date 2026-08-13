# ADR-0037: Real directory stage working trees instead of stage overlays

## Status

Accepted

## Context

ADR-0036 mounted each stage's OverlayFS as `/build` inside the sandbox
(lowerdir = merged `base/`, upperdir = `layers/NN-stage/`).  Within a day of
release, building wright itself failed with:

```
error: failed to create directory `/build/source/wright-5.5.1/target`
Caused by: Invalid cross-device link (os error 18)
```

Cargo creates `target/` via a temporary-directory rename
(`target<random>` → `target`).  OverlayFS rejects directory renames in the
vicinity of lower-layer directories with `EXDEV` unless the mount enables
`redirect_dir`, and `redirect_dir` stores its metadata in
`trusted.overlay.redirect` xattrs, which only `CAP_SYS_ADMIN` in the initial
user namespace may write.  Wright mounts stage overlays inside a user
namespace *by design* (including root runs), so the feature is unavailable:
`mount -o …,redirect_dir=on` fails with `EPERM`, and `userxattr` does not
lift the restriction (verified on Linux 7.1.2).  There is no mount-option
combination that makes directory renames work in an unprivileged overlay.

Directory renames are not an exotic cargo quirk — they are stock Unix
behavior that build scripts perform freely (`mv build.tmp build`,
autotools/cmake reshuffles, `patch` backups).  A working tree that cannot
rename directories near lower content is not a viable build environment.

## Decision

`/build` is a **real directory tree**, not an OverlayFS mount.

- Before a stage runs, `LayerManager::populate_target` populates `target/`
  from the merged `base/` — reflinks (copy-on-write) where the filesystem
  supports them (btrfs, xfs), hard links elsewhere, full copies as the last
  resort.
- The sandbox bind-mounts `target/` as `/build` (unchanged ADR-0013 system
  root overlay aside).  Unisolated stages keep working directly in
  `target/`.
- After the stage exits, `LayerManager::commit_layer` harvests the delta
  into `layers/NN-stage/` by diffing `target/` against `base/`, recording
  deletions as tombstones — the same mechanism pre-ADR-0036 fallback builds
  already used.

What does *not* change: the merged-base design of ADR-0036 (`base/`,
`.base_manifest`, `reconcile_base`, per-stage layers, tombstones, resume and
rewind semantics), the single-threaded isolation helper, and namespace
isolation itself.  The helper wire protocol moves to version 4
(`stage_overlay` removed); stale helpers fail closed.

Because hard links couple inodes across `base/`, layers, and the working
tree, an in-place write (`echo >> file`) through a hard-linked file would
silently rewrite every sibling and corrupt the layer record.  Reflinks are
therefore preferred wherever available: a copy-on-write clone keeps the
modification private to the working tree and visible to the diff.

## Alternatives

| Approach | Reason for rejection |
|----------|----------------------|
| `redirect_dir=on` | Requires `trusted.overlay.*` xattrs → init-namespace `CAP_SYS_ADMIN`.  User-namespace mounts reject it (`EPERM`), with or without `userxattr`; wright always mounts inside a user namespace. |
| Parent-mounted privileged overlays with `redirect_dir` | Reintroduces exactly what ADR-0036 removed: stale mounts after crashes and `EBUSY` superblock-teardown races.  Unavailable to unprivileged builds. |
| Keep stage overlays, avoid directory renames | Cargo itself renames to create `target/`; the restriction is incompatible with real-world build tooling. |
| Seed the upperdir as a full hard-link copy of `base/` (all parents pure-upper) | Same O(tree) populate cost as this ADR, plus overlay complexity and whiteout/opaque bookkeeping, for zero behavioral gain. |
| btrfs/ZFS snapshot backend | Already listed in ADR-0036 as a possible future opt-in; reflinks capture most of the benefit without tying the build path to specific filesystems. |

## Consequences

### Positive

- The whole `EXDEV` class is eliminated: the working tree is a plain
  directory, so renames, appends, and metadata operations behave exactly as
  on the host filesystem.
- No stage mounts at all: nothing to leak on crash, nothing to race at
  stage transitions; the `EBUSY` retry/`freshen_upper_layer` machinery is
  deleted rather than worked around.
- The same code path serves privileged, unprivileged, and `isolation =
  "none"` builds, ending the behavior split between overlay and fallback
  pipelines.
- On reflink-capable filesystems the pre-existing hard-link in-place-write
  caveat (documented in ADR-0036's consequences) is closed.

### Negative

- Each stage pays an O(tree) populate plus an O(tree) diff harvest instead
  of O(1) overlay capture.  With reflinks this is metadata-only on
  btrfs/xfs; on other filesystems it is the hard-link pass that
  pre-ADR-0036 fallback builds already paid.
- `merge_layer_tree` retains whiteout/opaque handling solely for reading
  layer directories written by overlay-based builds (≤ 5.5.1) during
  resume; new layers only contain plain trees and tombstones.
- The isolation helper wire protocol moves to version 4; stale helper
  binaries fail closed.

## References

- `crates/wright-engine/src/foundry/layers.rs` — merged base, populate,
  harvest, reflink sharing
- `crates/wright-engine/src/isolation/native/run.rs` — `/build` bind mount
- [ADR-0036](0036-merged-base-stage-overlays.md) — the superseded stage
  overlay design (merged-base bookkeeping survives)
- [ADR-0013](0013-multi-lowerdir-isolation.md) — sandbox system-root
  overlay (unchanged)
- Linux `Documentation/filesystems/overlayfs.rst` — `redirect_dir` and its
  xattr requirement
