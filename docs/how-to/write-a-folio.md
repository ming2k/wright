# How to Write a Folio

A **folio** is a declarative manifest naming the plans that form a
coherent system.  `wright launch` reads a folio, resolves and builds every
plan it names, and deploys the outputs into a target root.

For the full field list see [the reference](../reference/folio-manifest.md).

## Where Folios Live

Folios are bare TOML files that sit in their own directory, a **peer** of
the plans directory:

```
/var/lib/wright/
├── plans/                    # plan recipes
│   ├── glibc/plan.toml
│   ├── bash/plan.toml
│   └── nginx/plan.toml
└── folios/                   # system recipes
    ├── core.toml
    ├── desktop.toml
    └── container.toml
```

Plans recipes describe how to build a single part; folios describe how to
combine many parts into a system.  They are separate concerns and live in
separate trees.

When you reference a folio with `@core`, Wright searches:

1. `wright launch --folios <DIR>` (if given) — `<DIR>/core.toml`
2. `general.folios_dir` (defaults to `/var/lib/wright/folios`) — `<dir>/core.toml`

The first match wins.  Folios are never searched under `plans_dir`.

## Minimal Folio

A folio requires only a name and a version.  Everything else is optional.

```toml
[folio]
name    = "core"
version = "1"
plans   = ["glibc", "bash", "coreutils"]
```

Every entry in `plans` must correspond to a plan directory under a plans
search dir.

## External Assumptions

Use `[[provide]]` to mark parts the target system already provides.
Wright records each entry in the target database before building any
plans, so dependency checks succeed without Wright attempting to deploy
those parts.

```toml
[[provide]]
name    = "linux"
version = "6.12.0"

[[provide]]
name    = "busybox"
version = "1.36"
```

Typical use cases:

- The **kernel** on a VPS or bare-metal install.
- A **host-provided toolchain** during LFS-style bootstrapping.
- A **pre-installed bootloader** (`grub`, `systemd-boot`).

## Hooks

Use `[[hook]]` to run shell commands after every plan has been built and
deployed.  Hooks run on the host with both `$WRIGHT_ROOT` and `$ROOT`
set to the target root path.  They are **not** sandboxed.

```toml
[[hook]]
stage  = "post-launch"
script = """
echo "wright" > $ROOT/etc/hostname
ln -sf ../usr/share/zoneinfo/UTC $ROOT/etc/localtime
echo "LANG=en_US.UTF-8" > $ROOT/etc/locale.conf
for svc in sshd ntpd; do
    ln -sf /etc/sv/$svc $ROOT/var/service/$svc
done
"""
```

Only `stage = "post-launch"` is recognised; any other value is a parse
error.  This keeps Wright init-system-agnostic — whether your target uses
runit, systemd, or OpenRC, the folio author controls the post-launch
behaviour.

## Testing a Folio

Test against a disposable target root before applying to the live system:

```bash
wright launch --root /mnt/test --folio ./folios/core.toml
wright launch --root /mnt/test --plans ./plans --folios ./folios @core
wright launch --root /mnt/test @core vim curl
```

`--dry-run` prints the deploy plan without touching the target root or
the database:

```bash
wright launch --root /mnt/test --plans ./plans @core --dry-run
```

## Composing Folios

A folios directory typically holds several manifests, each describing a
different system profile:

```
folios/
├── base.toml         # glibc, bash, coreutils — absolute minimum
├── core.toml         # base + openssl, curl, vim — usable shell
├── maintenance.toml  # make, gcc, binutils — build toolchain
├── desktop.toml      # wayland, pipewire, firefox — graphical
└── container.toml    # core minus container-inappropriate parts
```

Multiple folios can be layered into one target root in a single command:

```bash
wright launch --root /mnt/new --plans ./plans --folios ./folios @base @core @desktop
```

Their `plans`, `[[provide]]`, and `[[hook]]` blocks are merged in order.

## Self-Contained Targets

`wright launch` mirrors every plan source dir and every referenced folio
file into the target under `/var/lib/wright/`, and writes a fresh
`/etc/wright/wright.toml` that points at the target-local paths.  The
deployed system can therefore run `wright install`, `wright upgrade`, or
`wright launch` against itself with no reference to the host tree.

Re-running launch against the same root converges drift: unchanged plans
are skipped, changed plans are rebuilt, missing plans are added, and the
synced sources are refreshed.

## Full Desktop Example

```toml
[folio]
name        = "desktop"
version     = "1"
description = "Wayland-based desktop system"

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
name    = "linux"
version = "6.12.0"

[[hook]]
stage  = "post-launch"
script = """
echo "wright-desktop" > $ROOT/etc/hostname
ln -sf ../usr/share/zoneinfo/Asia/Shanghai $ROOT/etc/localtime
echo "LANG=en_US.UTF-8" > $ROOT/etc/locale.conf
for svc in sshd dbus elogind; do
    ln -sf /etc/sv/$svc $ROOT/var/service/$svc
done
"""
```
