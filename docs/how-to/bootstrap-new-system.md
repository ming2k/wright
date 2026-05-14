# How to Bootstrap a New System

`wright launch` fills an empty mount point with a complete, self-contained
Wright system.  This guide covers the end-to-end process: preparing the target,
choosing a launch strategy, executing the bootstrap, and verifying the result.

For the design rationale, see [Launch Design](../explanation/launch-design.md).

## Prerequisites

Before launching, prepare the target root yourself: partition the disk, format
it, and mount it (e.g. at `/mnt/new`).  `wright launch` does not partition
disks — it fills a mounted root.

If the target is hosted on infrastructure that supplies a kernel and bootloader
(VPS, container image), declare those as `[[provide]]` entries in your folio so
dependency checks pass without Wright trying to install them.

The host must have a working toolchain matching the target's architecture.
If the target uses a different libc or architecture, build a seed system on a
matching host first.

## Choose a Launch Strategy

| Strategy | Command | When to use |
|----------|---------|-------------|
| **Folio file** | `wright launch --root <root> --folio <file>` | The folio fully describes the system. One command, one artifact. |
| **Folio reference** | `wright launch --root <root> --plans <dir> @<name>` | The folio lives alongside plans. Composable: `@base @desktop`. |
| **Plan names** | `wright launch --root <root> --plans <dir> <names...>` | Ad-hoc or experimental target. No folio needed. |

## Strategy 1: Launch from a Folio File

A folio file (`folio.toml`) names every plan in the system, declares external
assumptions, and optionally sets hostname, timezone, locale, and services.
Point `--folio` at it directly:

```bash
wright launch --root /mnt/new --folio ./folios/core.toml
```

This is the simplest path: one file fully describes the target.

### Example folio for a minimal container

```toml
[folio]
name    = "container-base"
version = "2026.05"
arch    = "x86_64"
plans   = ["glibc", "bash", "coreutils", "sed", "gawk", "grep", "tar", "gzip", "openssl"]

[[provide]]
name    = "linux"
version = "6.12.0"

[config]
hostname = "container"
timezone = "UTC"
```

### Example folio for a bare-metal workstation

```toml
[folio]
name    = "desktop"
version = "1"
arch    = "x86_64"
plans   = [
    "glibc", "bash", "coreutils", "util-linux",
    "e2fsprogs", "eudev", "kmod", "procps-ng",
    "openssl", "curl", "wget",
    "mesa", "libdrm", "libinput", "libxkbcommon",
    "wayland", "wayland-protocols", "wlroots",
    "sway", "foot", "firefox",
]

[[provide]]
name    = "linux"
version = "6.12.0"

[config]
hostname = "wright-desktop"
timezone = "Asia/Shanghai"
locale   = "en_US.UTF-8"
services = ["sshd", "dbus"]
```

See [How to write a folio](write-a-folio.md) for the complete folio format.

## Strategy 2: Launch from a Plans Directory

When folios live alongside plan directories, use `--plans` with `@folio`
references.  This works well when a single plans tree holds multiple system
profiles.

```bash
# Launch the @core folio from a plans directory
wright launch --root /mnt/new --plans ./plans @core

# Compose multiple folios
wright launch --root /mnt/new --plans ./plans @base @desktop

# Mix folios with explicit plan names
wright launch --root /mnt/new --plans ./plans @core vim curl
```

If `--plans` is omitted, Wright uses the configured `plans_dir`:

```bash
wright launch --root /mnt/new @core
```

### How folios are discovered

When you write `@core`, Wright searches:

1. `<plans_dir>/folios/core.toml`
2. `<plans_dir>/core/folio.toml`

Flat files under `folios/` are the recommended convention.

## Strategy 3: Launch with Explicit Plan Names

For quick experiments or ad-hoc target roots, name the plans directly.
Wright resolves dependencies, computes forge waves, and deploys everything:

```bash
wright launch --root /mnt/new --plans ./plans bash coreutils glibc gcc
```

This path uses no folio — no assumptions, no post-install config.  It is
useful for testing a small set of plans in isolation before writing a folio.

## What Launch Does

Regardless of the strategy, `wright launch` runs the same sequence:

1. Refuses `/` as the target root.
2. Creates the target directory skeleton (`var/lib/wright/`, `etc/wright/`,
   `var/log/wright/`).
3. Redirects `build_dir` and `parts_dir` under the target root — no host
   pollution.
4. Copies plan directories and referenced folio manifests into the target by
   comparing mtime and size; removes entries in the target that no longer
   exist on the host.
5. Writes `/etc/wright/wright.toml` inside the target, pointing all paths at
   target-local directories.
6. Pre-registers `[[provide]]` entries from folios in the target database.
7. Drives the full `resolve → build → seal → deploy` pipeline wave by wave.
8. Applies `[config]` (hostname, timezone, locale, runit services).

After launch, the target has a fully populated `wright.db`.  Running
`wright list --root /mnt/new` shows every installed part with its origin.

## Dry-Run First

Before a full launch, verify what would happen:

```bash
wright launch --root /mnt/new --folio ./folios/core.toml --dry-run
wright launch --root /mnt/new --plans ./plans @core --dry-run
```

The dry-run prints the deploy order, the plans that would be forged, and the
assumptions and config that would be applied — without writing any files.

## Re-Running Launch (Convergence)

`wright launch` is convergent.  If a previous run was interrupted or you
changed a plan on the host, re-run the same command:

```bash
wright launch --root /mnt/new --folio ./folios/core.toml
```

- Plans already deployed and matching their source are **skipped**.
- Missing plans are **built and installed**.
- Changed plans are **rebuilt** (build → seal → deploy).
- Plan files in the target are **re-synced** if they differ from the host.
- Stale files in the target that no longer exist on the host are **removed**.

This means an interrupted launch (network failure, power loss, disk-full) is
recovered by re-running the same command.  The forger's stage-level
checkpointing means individual plans resume from their last completed stage.

## Forcing a Rebuild

To force every plan to rebuild and redeploy, even if already present:

```bash
wright launch --root /mnt/new --folio ./folios/core.toml --force
```

## Verifying the Result

After launch completes, verify the target from the host:

```bash
# List every installed part
wright list --root /mnt/new --long

# Full health check
wright doctor --root /mnt/new

# File integrity check
wright check --root /mnt/new
```

All three should report clean immediately after a successful launch.

## Inspecting the Target

The target is self-contained.  Examine it directly:

```bash
# The target has its own database
ls -lh /mnt/new/var/lib/wright/wright.db

# Plans were copied into the target
ls /mnt/new/var/lib/wright/plans/

# Folios were copied too
ls /mnt/new/var/lib/wright/folios/

# The target's own wright.toml
cat /mnt/new/etc/wright/wright.toml
```

## Booting Into the Target

After launch and verification, the target is ready to boot.  The exact steps
depend on the environment:

**Bare metal / VM:**

```bash
# Install a bootloader (Wright does not deploy one — assume it or install separately)
# Then configure fstab, reboot, and select the new entry in the boot menu.
```

**Container (chroot):**

```bash
chroot /mnt/new /bin/bash
wright list       # runs inside the target, using the target's own database
wright doctor
```

**Container image (OCI):**

```bash
tar -C /mnt/new -c . | docker import - my-image:latest
```

## When to Use `wright provide` Instead

`wright launch` is for filling a **fresh** target.  If Wright is being added to
an **existing** LFS-style system that you built by hand and you only need to
register what is already on disk, use `wright provide` directly:

```bash
wright provide --file /etc/wright/bootstrap.txt
```

See the [CLI reference](../reference/cli-reference.md#wright-provide-name-version).

## Replacing Provided Parts

If your folio declared `[[provide]]` entries (e.g. a host-supplied kernel) and
you later build a Wright-managed replacement, install it normally into the
target:

```bash
wright install --root /mnt/new linux
```

The provided record is replaced by a fully-managed part entry.

## Troubleshooting

**Launch refuses `/`:**
The target root must be a separate mount point.  Mount the target first:
`mount /dev/sda1 /mnt/new`.

**Build fails with "command not found":**
The host needs a working toolchain matching the target architecture.  If
cross-compiling, build a seed system on a matching host first.

**Dependency check fails on provided parts:**
Ensure every external part (kernel, bootloader, host toolchain) is listed in
`[[provide]]`.  Launch pre-registers them before any plan builds.

**Launch ran out of disk space:**
Free space on the target mount, then re-run the same command.  Completed waves
are not rebuilt; launch resumes where it stopped.

**Host `parts_dir` is polluted after launch:**
This should not happen — `build_dir` and `parts_dir` are redirected under the
target root.  If it does, check that `--root` was passed and is not `/`.
