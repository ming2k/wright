# How to Write a Folio

A **folio** is a declarative manifest that names the plans forming a coherent
system.  `wright launch` reads a folio, resolves and builds all named plans,
and deploys them into a target root.  Unlike the old pack format, a folio does
not reference pre-built archives — it lists only plan names.

## Where Folios Live

Folios sit inside a plans directory, either as a flat file under a `folios/`
subdirectory or nested inside a plan directory:

```
plans/
├── folios/             ← named manifests
│   ├── core.toml
│   ├── desktop.toml
│   └── container.toml
├── glibc/
│   └── plan.toml
├── bash/
│   ├── plan.toml
│   └── folio.toml      ← per-plan folio
└── nginx/
    └── plan.toml
```

The file is always named after the folio: `core.toml`, `desktop.toml`, etc.
When you reference a folio with `@core`, Wright searches:

1. `<plans_dir>/folios/core.toml`
2. `<plans_dir>/core/folio.toml`

Flat files in `folios/` are the recommended convention.  Per-plan `folio.toml`
files exist for backwards compatibility with single-plan launches.

## The Minimal Folio

A folio manifest requires only two things: a name and a version.  Everything
else is optional.

```toml
[folio]
name = "core"
version = "1"

plans = ["glibc", "bash", "coreutils"]
```

The `plans` field lists the plan names Wright will resolve and build.  Each
name must correspond to a plan directory under `plans_dir`.

## Naming and Metadata

The `[folio]` table defines the folio's identity.  The name is used for
discovery and reference; the version is a free-form string you control.

```toml
[folio]
name = "container-base"
version = "2026.05"
description = "Minimal container image"
arch = "x86_64"

plans = [
    "glibc",
    "bash",
    "coreutils",
    "sed",
    "gawk",
    "grep",
    "tar",
    "gzip",
    "openssl",
]
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Folio identifier. Used in `@name` references. |
| `version` | yes | Free-form version string. |
| `description` | no | Human-readable summary. |
| `arch` | no | Target architecture hint (informational). |
| `plans` | no | Plan names to forge and deploy. May be omitted if an `invokes` folio only adds assumptions or config. |

`version` is not compared or resolved — it is just a label.  Bump it when the
folio's plan list or configuration changes.

## Declaring External Assumptions

Use `[[provide]]` to mark parts the target system provides externally.  Wright
records these assumptions in the target database before building any plans, so
dependency checks pass even though Wright did not deploy those parts.

```toml
[[provide]]
name = "linux"
version = "6.12.0"

[[provide]]
name = "busybox"
version = "1.36"
```

Common use cases:

- The **kernel** on a VPS or bare-metal install — Wright cannot deploy it.
- A **host-provided toolchain** during LFS-style bootstrapping.
- A **pre-installed bootloader** (`grub`, `systemd-boot`).

Each `[[provide]]` entry requires both `name` and `version`.

## System Configuration

The optional `[config]` section applies declarative host settings after all
plans are deployed.  Wright writes or symlinks the corresponding files inside
the target root.

```toml
[config]
hostname = "wright"
timezone = "UTC"
locale = "en_US.UTF-8"
services = ["sshd", "ntpd"]
```

| Field | Target file | Effect |
|-------|-------------|--------|
| `hostname` | `/etc/hostname` | Written directly |
| `timezone` | `/etc/localtime` | Symlinked to `/usr/share/zoneinfo/<value>` |
| `locale` | `/etc/locale.conf` | Written as `LANG=<value>` |
| `services` | `/var/service/<name>` | Symlinked to `/etc/sv/<name>` (runit) |

All fields are optional.  Omit `[config]` entirely if you do not need
declarative host settings.

## Testing a Folio

Test a folio against a disposable target root before applying it to the live
system:

```bash
wright launch --root /mnt/test --plans ./plans @core
```

`@core` tells Wright to find and resolve the `core` folio.  You can pass
multiple folio references and mix them with plain plan names:

```bash
wright launch --root /mnt/test --plans ./plans @base @maintenance @desktop
wright launch --root /mnt/test --plans ./plans @core vim curl
```

The `--dry-run` flag prints the deploy order and configuration actions without
writing any files:

```bash
wright launch --root /mnt/test --plans ./plans @core --dry-run
```

## Multiple Folios for System Profiles

A single plans directory typically holds several folios, each describing a
different system profile:

```
plans/folios/
├── base.toml       # glibc, bash, coreutils — absolute minimum
├── core.toml       # base + openssl, curl, vim — usable shell
├── maintenance.toml # make, gcc, binutils — build toolchain
├── desktop.toml    # wayland, pipewire, firefox — graphical
└── container.toml  # core minus anything container-inappropriate
```

Folios can reference one another.  To pull in the `base` folio's plans from
`core`, include `base`'s plans directly or use a meta-folio that references
both at launch time:

```bash
wright launch --root /mnt/new --plans ./plans @base @core
```

This launches both `base` and `core` into the same target root in a single
pass.

## Self-Contained Targets

`wright launch` copies the folio manifest and all referenced plan files into
the target root under `/var/lib/wright/`.  This makes the target
self-maintaining: you can `wright install`, `wright upgrade`, or re-run
`wright launch` directly inside the target (or chrooted into it) without the
host's plans directory.

When you re-run `launch` against the same root, Wright compares source files
to the copies in the target (by size and mtime) and only copies the ones that
changed.  Re-running `launch` on an already-provisioned root converges drift
rather than erroring.

## Example: Full Desktop Folio

```toml
[folio]
name = "desktop"
version = "1"
description = "Wayland-based desktop system"
arch = "x86_64"

plans = [
    # Core
    "glibc", "bash", "coreutils", "util-linux",
    "e2fsprogs", "eudev", "kmod", "procps-ng",
    # Networking
    "openssl", "curl", "wget",
    # Graphics
    "mesa", "libdrm", "libinput", "libxkbcommon",
    # Wayland
    "wayland", "wayland-protocols", "wlroots",
    # Desktop
    "sway", "foot", "firefox",
    # Fonts
    "fontconfig", "freetype", "noto-fonts",
]

[[provide]]
name = "linux"
version = "6.12.0"

[config]
hostname = "wright-desktop"
timezone = "Asia/Shanghai"
locale = "en_US.UTF-8"
services = ["sshd", "dbus", "elogind"]
```
