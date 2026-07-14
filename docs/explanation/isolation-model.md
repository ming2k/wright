# Isolation Model

Wright build stages can run with no isolation, relaxed isolation, or strict
isolation.  Strict isolation is the default because a package build should see a
predictable build root, write only to its assigned workspace, and avoid
modifying the host system by accident.

The current strict design is multi-lowerdir OverlayFS with a per-task writable
upper layer.  It replaced the older pre-copied sysroot design recorded in
ADR-0012.  ADR-0013 is the accepted decision record for the current approach.

## Isolation Levels

| Level | Root view | Intended use |
|-------|-----------|--------------|
| `none` | host root | debugging a broken plan or running a deliberately host-integrated stage |
| `relaxed` | OverlayFS root with host network and IPC | filesystem and process isolation for stages that require network access |
| `strict` | OverlayFS root from system lowerdirs plus task-private mounts | normal builds |

Wright resolves the effective policy once for execution, checkpointing, and
part provenance. A stage-level `isolation` override has highest priority,
followed by the selected executor's non-empty `default_isolation`, then the
global `build.default_isolation`. The built-in shell executor inherits the
global default.

## Failure Policy

Isolation fails closed. `relaxed` and `strict` never fall back to direct host
execution when a required namespace is unavailable. The stage stops before
its command starts and reports that `none` must be selected explicitly if
host execution is intended.

Every isolated process enters a user namespace, including when Wright runs as
root. Capabilities granted inside that namespace do not grant capabilities in
the parent namespace. The process also cannot gain new privileges when it
executes set-user-ID programs or files with capabilities.

This boundary reduces the effect of a compromised build process, but it does
not turn package builds into virtual machines. Kernel vulnerabilities and
resources deliberately shared by the selected isolation level remain part of
the threat model.

## Process Boundary

The main application does not perform namespace and mount setup in a child of
its multithreaded runtime. It starts a fresh copy of the Wright executable,
which enters an internal helper mode before creating worker threads. The
single-threaded helper constructs the namespace and supervises its init
process, while the application retains timeout, cancellation, output capture,
and logging responsibilities.

Helper startup, request validation, and namespace setup all fail closed. A
helper or protocol failure cannot turn an isolated stage into direct host
execution. The helper is an internal process boundary, not a separately
versioned user-facing command.

## Strict Root Construction

For the common case where the base root is `/`, strict isolation builds the
OverlayFS lower layer from host system directories:

```text
/usr
/bin
/sbin
/lib
/lib64
```

The paths are canonicalized, duplicate targets are removed, and subdirectories
already covered by a parent are dropped.  On a merged-/usr host, for example,
`/bin` usually resolves under `/usr`, so `/usr` alone covers that tree.

The resulting mount looks like:

```text
mount -t overlay overlay \
  -o lowerdir=/usr:/lib64:/lib,upperdir=<task>/upper,workdir=<task>/work \
  <task>/root
```

Every build task gets a distinct scratch directory:

```text
<build_dir>/.wright-isolation/<task_id>/
|-- root/
|-- upper/
`-- work/
```

The lower layers are read-only through OverlayFS.  If a build unexpectedly opens
a system path for writing, OverlayFS copies that file into the task's private
upper layer.  That write does not affect the host and does not affect another
parallel task.

## Task Mounts

After the OverlayFS root is mounted, Wright bind-mounts the task-specific
workspaces into the new root:

```mermaid
flowchart LR
    Work["host work/"] -->|read-write| Build["/build"]
    Staging["host staging/"] -->|read-write| Output["/output"]
    Deps["dependency outputs"] -->|read-only| Mounts["declared dependency mount points"]
```

It also mounts or bind-mounts the runtime pieces needed for ordinary build
tools:

- `/proc`
- `/dev`
- tmpfs `/run`
- tmpfs `/tmp`
- selected read-only `/etc` files such as `ld.so.cache`, `resolv.conf`,
  `hosts`, `passwd`, `group`, and `/etc/ssl`

The process then pivots into the new root, clears the inherited environment,
sets a small default environment, changes directory to `/build`, and executes
the stage script.

## Merged-/usr Fix

OverlayFS lowerdir canonicalization can flatten a merged-/usr hierarchy.  When
the lowerdir collapses to `/usr`, paths such as `/usr/lib` can appear at `/lib`
inside the overlay root, leaving no visible `/usr` directory.

That breaks programs that consult the host `ld.so.cache`, because the cache can
contain absolute paths like `/usr/lib/libreadline.so.8`.  Wright detects a
missing `/usr` after the overlay mount and bind-mounts host `/usr` read-only at
`/usr` inside the isolation root.

## Why OverlayFS Is Used

Earlier bind-mount based designs exposed shared host or sysroot inodes directly
to every parallel task.  That made ETXTBSY failures possible when many tasks
execed the same interpreter through shebang resolution.

OverlayFS changes the failure surface:

- writes through the overlay path go to the private upper layer
- unexpected system writes do not mutate shared lower inodes
- copy-up gives the task a private inode after write contention
- `/build` and `/output` are task-private bind mounts

There is still an edge case where a host process briefly holds a write reference
to a lower-layer inode at the exact moment a build task tries to execute it.
Wright handles that with ETXTBSY retry logic at both the isolation exec layer
and the pipeline stage layer.  Contributor details are in
[Isolation Race Handling](../dev/isolation-pitfalls.md).

## Recovering From a Crashed Run

If a Wright process is killed unexpectedly (SIGKILL, panic, OOM, power loss)
mid-build, an overlayfs mount can remain active in the kernel mount table
at `<build_dir>/<plan>-<version>/target` even though no Wright process holds
it. A subsequent `wright install` would then fail when trying to wipe the
build directory because the kernel returns `EBUSY` against a path that is
itself a mount point.

Wright recovers from this automatically. The first command after the crash
reads `/proc/self/mounts`, finds any mount targets that fall inside the
forge directory it's about to clean, lazy-unmounts them (`MNT_DETACH`,
deepest-first so parents become free), and retries the cleanup. The user
sees no warning — recovery is silent because nothing is wrong; the next
build proceeds normally.

You can still recover manually if the automatic cleanup somehow fails:

```bash
sudo umount -R /var/tmp/wright/workshop/<plan>-<version>/target
sudo rm -rf /var/tmp/wright/workshop/<plan>-<version>
```

Contributor implementation details are in
[Isolation Race Handling — EBUSY on cleanup](../dev/isolation-pitfalls.md).

## Relationship to ADRs

The current design decisions are:

- [ADR-0013: Multi-lowerdir OverlayFS isolation](../adr/0013-multi-lowerdir-isolation.md)
- [ADR-0027: Isolation fails closed](../adr/0027-isolation-fails-closed.md)
- [ADR-0028: Single-threaded isolation helper](../adr/0028-single-threaded-isolation-helper.md)

Historical context:

- ADR-0010 used a pre-copied read-only sysroot with bind mounts.
- ADR-0012 returned to OverlayFS with a pre-copied sysroot lower layer.
- ADR-0013 removed the pre-copy and uses host system directories as lowerdirs.
