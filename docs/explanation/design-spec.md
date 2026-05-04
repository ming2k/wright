# Design Specification

This document replaces the older historical spec. Wright is now a
source-first, local-first system with one primary CLI, distinct build/system
subcommands, and a single state database.

## Core Objects

- `plan`: the source definition for one buildable unit
- `part`: a built `.wright.tar.zst` archive
- `system`: the installed live state tracked in `wright.db`

## Tool Boundaries

- `wright build` builds parts from plans and creates `.wright.tar.zst` archives
- `wright package` slices staging directories into output directories (`outputs/`) and packages them into `.wright.tar.zst` archives
- `wright install` installs locally available archives to the live system
- `wright apply` resolves, builds, and installs plans in dependency waves

The main workflows are:

```bash
wright build curl
wright package curl
wright install ./curl-8.0-1-x86_64.wright.tar.zst

# Or the all-in-one apply workflow:
wright apply curl
```

## File Model

A `plan.toml` lives in its own directory under `plans_dir`. Each plan is self-contained:

```
plans/curl/plan.toml
```

## Output Model

Each build produces one or more `.wright.tar.zst` archives under `parts_dir`. A plan
can have multiple outputs (e.g. `gcc` and `gcc-libs`) defined by `[[output]]` tables.

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
wright install →  install archives
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
| `strict` | PID, mount, UTS, IPC, net, user (when unprivileged) | **Pre-copied read-only sysroot** | Full sandbox; default and recommended |

### Why strict isolation uses a pre-copied sysroot

Earlier designs used Linux OverlayFS with `lowerdir=/` as the sandbox root.
This caused reproducible `ETXTBSY` ("Text file busy") failures when multiple
parallel tasks executed shebang scripts (`./configure`, `make`, etc.) because
they shared the host's live inode cache.

The current design copies `/usr`, `/bin`, `/lib` and essential `/etc` files into
`/var/tmp/wright/sysroot/` once, makes the tree read-only (`chmod -R a-w`), and
mounts that directory as the root for every strict-isolation task.  Because the
copied inodes are never opened for writing by any host process, the kernel
never raises `ETXTBSY`.

```text
Host
│
├─ /var/tmp/wright/sysroot/           ← created once, read-only, reused
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
        │           ├─ mount --rbind /var/tmp/wright/sysroot → /tmp/.../root
        │           ├─ mount -o remount,ro root
        │           ├─ mount tmpfs → root/tmp, root/run
        │           ├─ bind mount work/  → root/build   (rw)
        │           ├─ bind mount output/ → root/output (rw)
        │           ├─ pivot_root
        │           └─ execve("/bin/sh", ...)
        └─ Task "expat"
            └─ (same flow, same sysroot, independent mount namespace)
```

**Concurrency:** the first task to need the sysroot acquires an `flock(LOCK_EX)`
on `sysroot.lock`, performs the copy, and releases the lock.  Other tasks block
and then reuse the result.  The copy is invalidated and rebuilt automatically
when the host system directories have newer mtimes.

This approach is filesystem-agnostic (works on ext4, btrfs, xfs, tmpfs) and
requires no external tools (`mksquashfs`, `btrfs`, etc.).  See
[ADR-0010](../adr/0010-pre-copied-sysroot-isolation.md) for the full rationale
and rejected alternatives.

## Concurrency Model

Builds run in dependency-ordered waves. Plans in the same wave build in parallel.
The scheduler divides available CPUs across active isolations.
