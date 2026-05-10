# ADR-0014: `wright launch` and the Pack Format

## Status

Superseded by [ADR-0015](0015-group-manifest-replaces-pack.md)

## Context

Until now, Wright's bootstrap story for a brand-new machine assumed an LFS-style
hand-built base system already existed. The user installed Wright onto that
system and ran `wright assume` to register what was already there. That works
for the maintainer of an LFS host but does not answer the more common question:

> "I just got Wright, I have plans (or a packaged set of parts), and a bare
> target root. How do I get a working system on it?"

The obvious reference point is Arch's `pacstrap`: mount the target, set up
pacman state inside it, install the `base` group, chroot in, configure. We did
not want to copy that pattern verbatim because it bakes in several assumptions
that do not match Wright:

- `pacstrap` is imperative and not re-runnable: a half-finished bootstrap is
  generally discarded, not converged.
- Distribution is per-package + a repo + a keyring. There is no first-class
  "this is a coherent base system" artifact.
- Configuration (hostname, locale, services) is "now chroot in and run a
  series of commands," not data.
- Origin information (manual vs dependency vs externally provided) is lost on
  the new system; everything pacstrapped looks manual.
- Source-first installs are not a first-class flow.

Wright already has the primitives to do better. `install_part_with_origin`
takes a `root_dir`. Hooks chroot when `root_dir != /`. `apply` already runs
wave-by-wave install with origin tracking. The DB path is `--db`-overridable.
What is missing is a portable input that says "this is a base system" and a
single user-facing command that performs the initial fill.

## Decision

Add two pieces:

1. **The pack format**: a single `.wright.pack.tar` artifact containing a
   `pack.toml` manifest, the parts archives it references, an optional
   `overlay/` tree of base configuration, and an optional list of assumed
   externals. A pack is the unit of distribution for "a system you can
   bootstrap from."

2. **`wright launch`**: a top-level command that converges a target root from
   a pack (binary path) or from plans (source path). Both paths share the same
   transaction code that the existing live-system commands use.

### Pack format

```
base.wright.pack            (uncompressed tar)
├── pack.toml               # manifest
├── parts/                  # *.wright.tar.zst, content-addressed by name
└── overlay/                # optional /-rooted skeleton (fstab, hostname, ...)
```

`pack.toml` schema:

```toml
[pack]
name        = "wright-base"
version     = "2026.05"
description = "Wright minimal base system"
arch        = "x86_64"

# Parts shipped inside this pack. Order is informational; install order is
# computed from the dependency graph at launch time.
[[part]]
file   = "parts/glibc-2.41-1-x86_64.wright.tar.zst"
origin = "manual"          # manual | dependency

[[part]]
file   = "parts/libgcc-14.2.0-1-x86_64.wright.tar.zst"
origin = "dependency"

# Externals the target is expected to provide (e.g. host-supplied kernel on a
# VPS). Recorded via assume_part before installing.
[[assume]]
name    = "linux"
version = "6.12.0"

# Optional declarative system configuration applied after the install waves.
[config]
hostname = "wright"
timezone = "UTC"
locale   = "en_US.UTF-8"
services = ["sshd"]            # runit symlinks under /var/service/
```

The whole pack is content-addressed: `pack.toml` carries SHA-256 hashes of each
part archive and the overlay tar, so post-install verification is straightforward
and the pack is signable as a single artifact.

### `wright launch`

```
wright launch --root /mnt/new <SOURCE>
```

Where `<SOURCE>` is one of:

- A path to a `.wright.pack.tar` file (binary path).
- `--plans <DIR>` plus one or more plan names (source path; reuses `apply`).
- `--profile <NAME>` (later; resolves to a pack via configured pack search dirs).

Launch responsibilities:

1. Initialize `<root>/var/lib/wright/` (db file, parts dir, lock dir) and open
   a fresh `InstalledDb` rooted there.
2. Record `[[assume]]` entries via `assume_part` so dependency checks pass.
3. Compute install order from declared part archives (DAG sort, same path as
   `install_parts_with_explicit_targets`).
4. Install each wave into `--root` using the existing transaction code, with
   `Origin::Manual` or `Origin::Dependency` per the manifest.
5. Extract the overlay tree (if any) into `--root`, refusing to clobber files
   already owned by an installed part.
6. Apply `[config]` (write `/etc/hostname`, set timezone symlink, enable runit
   services).
7. Print a summary and exit. The target is now a coherent Wright system whose
   `wright list` matches the pack.

Re-running `wright launch` on the same root is a convergence operation, not an
error: already-installed matching parts are skipped, missing ones are added,
mismatched ones are upgraded.

## Consequences

**Wins.**

- One artifact = one bootstrappable system. Mirrors, signing, and integrity
  checks all operate on a single file.
- Re-runnable. A failed launch (network, disk, power) is fixed by re-running,
  not by starting over.
- Origin-aware from day one. The new system's database does not start with
  every part marked `manual`.
- Source and binary paths converge through the same transaction layer; no
  duplicate code path.
- Assumed externals are first-class in the manifest, so VPS/cloud scenarios
  ("the provider gives me a kernel") are expressible.

**Costs and risks.**

- `--root` plumbing must be audited for any remaining hardcoded `/` paths,
  particularly in lock-dir derivation, parts dir resolution, and any hook that
  shells out to host binaries that should run inside the chroot.
- Pack creation tooling (`wright pack`) is additional surface area to maintain.
- `[config]` overlap with future configuration management. We deliberately
  keep the supported keys small (hostname, timezone, locale, services) so the
  pack stays a bootstrapping artifact, not a config-management system.

**Non-goals.**

- Pack does not replace a repository. A pack is a frozen snapshot. A rolling
  repository is a separate, later concern.
- Launch is not a system installer in the disk-partitioning sense. The user
  prepares the target root (mount point); Wright fills it.
- A pure-target install with no host Wright (live-ISO scenario) is out of
  scope for the first iteration. The host builds or provides; the target
  receives.

## Relation to existing ADRs

- Builds on [ADR-0002](0002-wave-by-wave-install.md): launch reuses the
  wave-by-wave install machinery; a pack install is just a single wave (or
  multiple waves when the dependency graph requires it) with `root_dir` set.
- Builds on [ADR-0005](0005-two-database-design.md): launch initializes the
  installed-parts database inside the target root, leaving the host's database
  untouched.
- Aligns with [ADR-0004](0004-no-magic-behavior.md): launch is opinionated but
  not magical — the manifest is explicit, the install order is derivable, and
  every step is inspectable.
