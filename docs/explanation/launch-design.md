# Launch Design

`wright launch` fills a target root with a coherent, origin-aware Wright system.
It exists as a peer to `wright install`, not a thin wrapper — it solves a
fundamentally different problem: bootstrapping a system from scratch into an
empty mount point.

## Mission

The mission of `wright launch` is **provisioning**: given a folio manifest (or a
set of plan names) and an empty target root, produce a self-contained,
self-maintaining Wright system inside that root.  The resulting system carries
its own database, its own copy of every plan and folio it was built from, and a
`wright.toml` that points at its own local directories — it needs nothing from
the host to continue operating.

## Why a Separate Command

`wright install` assumes a **live system**: the target root is `/`, the database
already exists, and Wright itself was probably installed by a prior Wright
invocation.  `wright launch` starts from nothing — no database, no directory
skeleton, no configuration, and a target root that may not even be bootable yet.

| Concern | `wright install` | `wright launch` |
|---------|------------------|-----------------|
| Target root | `/` by default | Must be explicit; refuses `/` |
| Database | Exists, shared | Created fresh inside target |
| Build outputs | Land on host | Redirected into target |
| Plan sources | Read from host | Copied into target for self-maintenance |
| External assumptions | Already registered | Registered from folio before build |
| Post-install config | Not applied | Applied from folio `[config]` |

The refusal to target `/` is deliberate.  Overwriting a running system's root
with a fresh bootstrap would corrupt the live system.  `launch` always operates
on a separate mount point.

## Operational Modes

`wright launch` has two operational modes, unified by the same core pipeline:

### Folio mode (`--folio`)

A single `folio.toml` drives the entire bootstrap.  The folio names the plans,
declares external assumptions, and optionally specifies post-install
configuration.  This is the recommended path: one file fully describes the
target system.

```bash
wright launch --root /mnt/new --folio ./folios/core.toml
```

### Plans mode (`--plans` or positional targets)

Plan names and `@folio` references are resolved from a plans directory.  This is
the path for experimentation and ad-hoc target roots.

```bash
wright launch --root /mnt/new --plans ./plans bash coreutils glibc
wright launch --root /mnt/new --plans ./plans @core @desktop
wright launch --root /mnt/new @core               # uses default plans_dir
```

### What happens step by step

1. **Refuse `/`** — if the target root is `/`, abort immediately.

2. **Resolve** — load the folio (or expand `@folio` references against the
   plan search dirs), producing the merged plan list, the `[[provide]]`
   entries, the `[[hook]]` entries, and the set of folio files that need
   to be mirrored into the target.

3. **Dry run shortcut** — if `--dry-run` was passed, print the resolved
   plan list, provides, and hook count, then exit without touching the
   target root or any database.

4. **Skeleton** — create the target directory layout:
   `var/lib/wright/{parts,store,staging,lock,plans,folios,sources}`,
   `var/log/wright`, `var/tmp/wright`, `etc/wright`.

5. **Redirect artefact paths** — clone the global config and rewrite
   `parts_dir`, `store_dir`, `source_dir`, `logs_dir`, and `forge_dir` to
   point inside the target root.  Plan and folio source dirs stay on the
   host (their contents are mirrored in the next step).  Build outputs,
   sealed archives, source downloads, and logs all land in the target.

6. **Mirror plan and folio sources** — copy each plan directory and
   every referenced folio file into `<root>/var/lib/wright/plans/` and
   `<root>/var/lib/wright/folios/`.  Files are transferred only when they
   differ from the target by size or mtime; entries in the target that no
   longer exist on the host are removed.  The target stays a faithful
   mirror of the host's plan tree, restricted to the plans the folio
   asked for.

7. **Write target config** — generate `/etc/wright/wright.toml` inside
   the target, pointing every path at the target-local layout so the
   deployed system can run `wright install`, `wright upgrade`, or
   `wright launch` against itself.

8. **Pre-register assumptions** — insert each `[[provide]]` entry into
   the target's fresh `wright.db` so dependency checks pass without
   Wright attempting to deploy the kernel, host toolchain, or other
   externals.

9. **Build → Seal → Deploy** — drive the full `resolve → build → seal →
   deploy` pipeline, wave by wave, reusing `wright install`'s engine.
   Each completed wave is installed into the target before the next wave
   begins, so a plan's dependencies are already on disk when it enters
   its `configure` stage. **Plan-level deploy hooks (`pre_install`, `post_install`, etc.) are disabled during this phase**, as they are designed to run natively on the target system (which may not be bootable or compatible with the host environment during `launch`). System configuration must be handled by folio hooks instead.

10. **Execute hooks** — run every `[[hook]]` from the folio.  Hooks
    execute on the host under `sh -c` with both `$WRIGHT_ROOT` and
    `$ROOT` set to the target root path.  Only `stage = "post-launch"`
    is recognised; any other stage is a parse error.  Hooks are the
    extension point for system configuration that cannot be expressed
    through plan deployment (hostname, locale, service enablement,
    bootloader installation, …).  Hooks are not sandboxed; they run with
    the same privileges as `wright`.

## Convergence

`wright launch` is **convergent**.  Re-running against the same target root does
not error or duplicate — it converges drift:

- Plans that are already deployed and match their source definition are skipped.
- Missing plans are built and installed.
- Changed plans are rebuilt (build → seal → deploy).
- Plan and folio files in the target are re-synced if they differ from the host.
- Assumed parts already registered are not duplicated.

This makes launch **re-runnable**.  An interrupted launch (network failure,
power loss, disk-full) is recovered by re-running the same command.  The
foundry's stage-level checkpointing means individual plans resume from their last
completed stage rather than restarting from scratch.

## Root Isolation

Every artefact produced by launch lives inside `--root`.  This has three
consequences:

1. **No host pollution.**  The host's `parts_dir`, `forge_dir`, and `wright.db`
   are never touched.  Running `wright launch --root /mnt/a @core` and
   `wright launch --root /mnt/b @desktop` in parallel cannot collide.

2. **The target is self-contained.**  After unmounting and booting into it, the
   target can run `wright install`, `wright upgrade`, or `wright launch`
   directly.  Its plan tree, folios, part store, and database are all local.

3. **Clean teardown.**  Removing the mount point removes every trace of the
   bootstrap.  No host-side databases or archives need manual cleanup.

## Relationship to the Ship of Theseus Metaphor

In Wright's metaphor, the live system is the ship that keeps sailing while
parts are replaced.  `wright launch` is the shipyard — it constructs a new ship
that can sail independently.  Once launched, the new system becomes a peer
ship, maintained through the same `install`, `upgrade`, and `remove` commands.

## Folio Manifest as the Source of Truth

The folio manifest (`folio.toml`) is the single declarative file that describes
everything needed to bootstrap a system.  It replaces the earlier pack format
(see [ADR-0015](../adr/0015-folio-manifest-replaces-pack.md)) which bundled
pre-built archives — a folio is a build recipe, not a binary bundle.  This means:

- The folio stays current as plans evolve; no separate archive-rebuild step.
- The same folio can produce a system for any architecture by rebuilding.
- Folios compose: `wright launch --root /mnt/new @base @desktop` layers two
  manifests into one target root.

## When Not to Use Launch

- **Adding Wright to an existing system.**  If you already have a hand-built
  LFS system and only need to register what is on disk, use `wright provide`.
  Launch expects an empty target root.

- **Installing or upgrading parts on a live system.**  Use `wright install` or
  `wright upgrade`.  Launch refuses to target `/` and recreates the database.

- **Just building plans.**  Use `wright build`.  Launch is a full bootstrap
  pipeline, not a build tool.

## Related

- [ADR-0014](../adr/0014-launch-and-pack-format.md) — Original launch + pack design (superseded)
- [ADR-0015](../adr/0015-folio-manifest-replaces-pack.md) — Folio manifest replaces pack format
- [How to bootstrap a new system](../how-to/bootstrap-new-system.md)
- [How to write a folio](../how-to/write-a-folio.md)
- [Folio manifest reference](../reference/folio-manifest.md)
- [Execution hierarchy](execution-hierarchy.md) — Where launch fits in the three-tier metaphor
- [Architecture](architecture.md) — Overall system architecture
