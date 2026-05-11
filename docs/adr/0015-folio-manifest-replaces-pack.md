# ADR-0015: Folio Manifest Replaces Pack Format

## Status

Accepted

## Context

[ADR-0014](0014-launch-and-pack-format.md) introduced the `.wright.pack.tar`
format and `wright launch` as a way to bootstrap a target root from a bundle of
pre-built archives.  After using the pack format in practice we identified two
structural limitations:

1. **No collection abstraction.**  Users had to list every plan name on the
   command line (`wright launch --plans ./plans bash coreutils glibc ...`).
    There was no way to say "install the `@core` folio" and have Wright resolve
   the set automatically.

2. **No true host isolation.**  When `launch --root /mnt/new` built from plans,
   the build outputs (`build_dir`, `parts_dir`) still landed on the host
   filesystem.  Parallel bootstraps of multiple target roots, or a target whose
   mount point differed from the host layout, could collide or leave stale
   artefacts behind.

The pack format solved distribution (a single file containing archives + manifest)
but did not solve either of the above problems.  Moreover, maintaining
`wright pack` (bundling, overlay extraction, integrity verification) added
surface area without giving us a source-first bootstrap path.

We want:
- A declarative way to name a *set* of plans.
- A command that resolves, builds, packages, and installs that set into a target
  root with zero host pollution.
- No extra artifact format to maintain.

## Decision

1. **Remove the pack format** (`wright pack`, `.wright.pack.tar`, `pack.toml`).
2. **Introduce the folio manifest** (`folio.toml`) — a pure declaration that
   names plans, assumed externals, and optional post-install configuration.
3. **Extend `wright launch`** to accept either `--folio <file>` or `--plans <dir>`
   (with optional `@folio` references), drive the full
   `resolve → build → package → install` pipeline, and automatically redirect
   `build_dir` and `parts_dir` under the target root.

### Folio manifest (`folio.toml`)

```toml
[folio]
name        = "wright-base"
version     = "2026.05"
description = "Wright minimal base system"
arch        = "x86_64"
plans       = ["glibc", "bash", "coreutils", "openssl"]

[[assume]]
name    = "linux"
version = "6.12.0"

[config]
hostname = "wright"
timezone = "UTC"
locale   = "en_US.UTF-8"
services = ["sshd"]
```

- `plans` — names of plans to resolve, build, and install.
- `[[assume]]` — externals pre-registered before any build starts.
- `[config]` — declarative post-install system configuration.

Unlike `pack.toml`, a folio does **not** reference pre-built archives or carry
an overlay tree.  It is a build recipe, not a binary bundle.

### `wright launch` changes

```bash
# From an explicit folio file
wright launch --root /mnt/new --folio ./folios/core.toml

# From a plans directory with explicit plan names
wright launch --root /mnt/new --plans ./plans bash coreutils glibc

# From a plans directory using a folio reference
wright launch --root /mnt/new --plans ./plans @core
```

When `--root` is set (and is not `/`):
- `build_dir` is redirected to `<root>/var/tmp/wright/workshop`
- `parts_dir` is redirected to `<root>/var/lib/wright/parts`

In addition, `launch` copies all source plans into
`<root>/var/lib/wright/plans/` and any referenced folio manifests into
`<root>/var/lib/wright/folios/`, then writes a minimal
`/etc/wright/wright.toml` that points at the target-local directories.
This makes the resulting root self-contained: after unmounting and booting
into it, the user can run `wright apply @core` directly on the target
without needing access to the host plan tree.

This guarantees that all build artefacts live inside the target root and that
the target can maintain itself independently.

## Consequences

**Wins.**

- One manifest = one bootstrappable system.  No need to maintain pre-built
  archive bundles; the folio stays current as plans evolve.
- Re-runnable.  A failed launch is fixed by re-running, not by starting over.
- Origin-aware from day one via the existing `apply` workflow.
- Source-first installs are first-class: launch builds from source when archives
  do not yet exist.
- Full host isolation: multiple parallel target roots do not interfere.
- `@folio` references let users compose systems without enumerating every plan.

**Costs and risks.**

- Building from source on every launch is slower than installing pre-built
  archives.  Users who want speed can run `wright build` + `wright package`
  once, then use `wright install --path` with the resulting archives.
- `--root` plumbing must still be audited for any remaining hardcoded `/` paths.
- `[config]` deliberately stays small (hostname, timezone, locale, services) to
  avoid becoming a configuration-management system.

## Relation to existing ADRs

- Supersedes [ADR-0014](0014-launch-and-pack-format.md).
- Builds on [ADR-0002](0002-wave-by-wave-install.md): launch reuses the
  wave-by-wave install machinery.
- Builds on [ADR-0005](0005-two-database-design.md): launch initialises the
  target database inside `--root`, leaving the host database untouched.
