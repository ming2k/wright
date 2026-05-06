# Design Specification

This document replaces the older historical spec. Wright is now a
source-first, local-first system with one primary CLI, distinct build/system
subcommands, and a single state database.

## Core Objects

- `plan`: the source definition for one buildable unit
- `part`: a built `.wright.tar.zst` archive
- `system`: the installed live state tracked in `wright.db`

## Tool Boundaries

- `wright build` builds plans into staging and output directories
- `wright package` slices staging directories into output directories (`outputs/`) and packages them into `.wright.tar.zst` archives
- `wright install` installs locally available plan outputs to the live system
- `wright apply` resolves, builds, and installs plans in dependency waves

The main workflows are:

```bash
wright build curl
wright package curl
wright install curl

# Or the all-in-one apply workflow:
wright apply curl
```

## File Model

A `plan.toml` lives in its own directory under `plans_dir`. Each plan is self-contained:

```
plans/curl/plan.toml
```

## Output Model

Each package step produces one or more `.wright.tar.zst` archives under `parts_dir`.
A plan can have multiple outputs (e.g. `gcc` and `gcc-libs`) defined by `[[output]]` tables.

## State Model

`wright.db` is the single source of truth for:

- installed parts and their files
- dependency relationships
- transaction history
- build/apply resume sessions

## CLI Architecture

```
wright build   →  build plans
wright package →  slice staging into outputs and package
wright apply   →  resolve + build + install
wright install →  install plan outputs
wright upgrade →  upgrade installed parts
wright remove  →  remove installed parts
wright list    →  list installed parts
wright resolve →  inspect dependency graph
wright lint    →  validate plan files
wright prune   →  clean old archives
```

## Isolation Model

Build stages run in optional sandboxed environments. The default isolation level
is `strict`. Each stage can override this via its `isolation` field.

### Isolation levels

| Level | Namespaces | Root filesystem | Use case |
|-------|------------|-----------------|----------|
| `none` | — | Host root | Debugging a broken plan; fastest but zero protection |
| `relaxed` | PID, mount, UTS | Host root via bind mounts | Basic process isolation; still sees live host filesystem |
| `strict` | PID, mount, UTS, IPC, net, user (when unprivileged) | **OverlayFS with shared read-only sysroot lower layer** | Full sandbox; default and recommended |

### Why strict isolation uses overlayfs with a shared sysroot lower layer

Earlier designs used Linux OverlayFS with `lowerdir=/` as the sandbox root.
This caused reproducible `ETXTBSY` ("Text file busy") failures when multiple
parallel tasks executed shebang scripts (`./configure`, `make`, etc.) because
they shared the host's live inode cache.

The current design copies `/usr`, `/bin`, `/lib` and essential `/etc` files into
`/var/tmp/wright/sysroot/` once, makes the tree read-only (`chmod -R a-w`), and
uses it as a shared overlayfs lower layer.  Each task gets its own writable
upper layer, so any file that becomes write-contended is automatically
copy-up'd to a task-private inode by the kernel.

```text
Host
│
├─ /var/tmp/wright/sysroot/           ← created once, read-only, shared lower
│   ├── bin/sh
│   ├── usr/bin/gcc
│   ├── lib/
│   └── etc/passwd
│
└─ wright apply
    └─ Batch 1 (parallel tasks)
        ├─ Task "bzip2"
        │   └─ fork → unshare(NEWNS|NEWPID|...)
        │       └─ fork → Grandchild (PID 1)
        │           ├─ mount -t overlay overlay
        │           │     -o lowerdir=/var/tmp/wright/sysroot,
        │           │        upperdir=.../upper,workdir=.../work
        │           │     → root
        │           ├─ mount tmpfs → root/tmp, root/run
        │           ├─ bind mount work/  → root/build   (rw)
        │           ├─ bind mount output/ → root/output (rw)
        │           ├─ pivot_root
        │           └─ execve("/bin/sh", ...)
        └─ Task "expat"
            └─ (same flow, same lower layer, per-task upper, independent mount namespace)
```

**Concurrency:** the first task to need the sysroot acquires an `flock(LOCK_EX)`
on `sysroot.lock`, performs the copy, and releases the lock.  Other tasks block
and then reuse the result.  The copy is invalidated and rebuilt automatically
when the host system directories have newer mtimes.

This approach is filesystem-agnostic (works on ext4, btrfs, xfs, tmpfs) and
requires no external tools (`mksquashfs`, `btrfs`, etc.).  See
[ADR-0012](../adr/0012-overlayfs-per-task-upper.md) for the full rationale
and rejected alternatives.

## Concurrency Model

Builds run in dependency-ordered waves. Plans in the same wave build in parallel.
The scheduler divides available CPUs across active isolations.
