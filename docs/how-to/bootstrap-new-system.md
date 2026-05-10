# How to Bootstrap a New System

Wright covers two starting points when filling a fresh target root: a packaged
**pack** of parts, or a directory of **plans**. Both go through `wright launch`
and produce a coherent, origin-aware Wright system on the target.

For the rationale behind the launch + pack design, see
[ADR-0014](../adr/0014-launch-and-pack-format.md).

## Prerequisites

Before launching, prepare the target root yourself: partition the disk, format
it, and mount it (e.g. at `/mnt/new`). `wright launch` does not partition
disks — it fills a mounted root.

If the target is hosted on infrastructure that supplies a kernel and bootloader
(VPS, container image), declare those as `[[assume]]` entries in your pack
manifest so dependency checks pass without Wright trying to install them.

## Path A: Launch from a Pack

A **pack** (`.wright.pack.tar`) is a single artifact that bundles every part
needed to fill a target, plus an optional `overlay/` configuration tree and a
small declarative `[config]` block. It is the simplest way to install a Wright
system on a fresh machine.

```bash
wright launch --root /mnt/new ./wright-base-2026.05.wright.pack.tar
```

What launch does:

1. Initializes `/mnt/new/var/lib/wright/` (database, parts dir, lock dir).
2. Records every `[[assume]]` entry from the pack so dependency checks pass.
3. Installs every `[[part]]` archive in dependency order, preserving the
   `origin` (manual or dependency) declared in the manifest.
4. Extracts the pack's `overlay/` tree into the target, refusing to clobber
   files owned by an installed part.
5. Applies the declarative `[config]` block (hostname, timezone, locale,
   runit service symlinks).

After launch, the target has a fully populated `wright.db`. `wright list
--root /mnt/new` will show every installed part with the right origin.

### Re-running launch

`wright launch` is convergent. If a previous run was interrupted (network,
power, disk-full), re-running on the same root resumes: parts that already
match the manifest are skipped, missing ones are added, mismatched ones are
upgraded.

### Inspecting a pack

```bash
wright pack inspect ./wright-base-2026.05.wright.pack.tar
```

prints the manifest, the part archives it carries, the overlay file count,
and the SHA-256 hashes used for verification.

## Path B: Launch from Plans (Source-First)

When you have a directory of plans rather than a prebuilt pack, point `launch`
at it with `--plans`. Wright builds in dependency waves on the host (using the
host's existing toolchain) and installs each completed wave into the target
root.

```bash
wright launch --root /mnt/new --plans ./plans bash coreutils glibc gcc
```

This reuses `wright apply`'s wave engine end-to-end. The only difference from
a normal `apply` is that installs target `--root` instead of `/`.

### Plan and group synchronisation

`launch` keeps the target's plan definitions in sync with the host so the
target can self-maintain later.  During every run it:

1. Compares each source plan file against the copy in
   `<root>/var/lib/wright/plans/` (by size and mtime).
2. Copies only the files that have changed.
3. Removes files or directories in the target that no longer exist on the
   host.

The same logic applies to group manifests in `<root>/var/lib/wright/groups/`.
This means you can edit a plan on the host and re-run `launch`; the target
will receive the updated definition without a full rebuild of plans that
have not changed.

Because the host builds, the host must already have a working toolchain. If
you are filling a target with a different libc or arch from the host, build a
seed pack from a host that matches first, then use Path A on the bare target.

## Path C: Drop a Pack into a Profile Directory

For repeatable installs, place packs in one of the configured `pack_dirs`
(default: `/var/lib/wright/packs`). Then refer to them by name:

```bash
wright launch --root /mnt/new --profile minimal
```

Wright resolves `minimal` to the newest matching pack in `pack_dirs`.

## Building a Pack

To create a pack from your own parts, write a `pack.toml` manifest in a
directory alongside a `parts/` subdirectory holding the archives, then run:

```bash
wright pack ./my-base/
```

A minimal `pack.toml`:

```toml
[pack]
name        = "my-base"
version     = "1"
description = "My minimal base system"
arch        = "x86_64"

[[part]]
file   = "parts/glibc-2.41-1-x86_64.wright.tar.zst"
origin = "manual"

[[part]]
file   = "parts/bash-5.2-1-x86_64.wright.tar.zst"
origin = "manual"

[config]
hostname = "wright"
timezone = "UTC"
```

`wright pack` walks the `[[part]]` list, verifies that each file exists,
records its SHA-256 in the manifest, optionally tars an `overlay/` directory,
and writes a single `.wright.pack.tar` artifact.

## When to Use `wright assume` Instead

`wright launch` is for filling a fresh target. If Wright is being added to an
**existing** LFS-style system that you built by hand and you only need to
register what is already on disk, use `wright assume` directly. See
[`wright assume` in the CLI reference](../reference/cli-reference.md#wright-assume-name-version).

```bash
# Existing system; only register what's already there.
wright assume --file /etc/wright/bootstrap.txt
```

## Verifying the Result

```bash
wright list --root /mnt/new --long
wright doctor --root /mnt/new
wright verify --root /mnt/new
```

`doctor` walks the new database for integrity, dependency, and shadow-file
issues. `verify` recomputes file hashes. Both should report clean immediately
after a successful launch.

## Replacing Assumed Parts

If your pack declared `[[assume]]` entries (e.g. a host-supplied kernel) and
you later build a Wright-managed replacement, install it normally:

```bash
wright install --root /mnt/new linux
```

The assumed record is replaced by a fully-managed part entry.
