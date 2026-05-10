# Group Manifest (`group.toml`)

Reference for the group manifest format used by `wright launch`.

## Overview

A group is a declarative list of plans that form a coherent system (e.g. a
minimal base system, a development workstation, or a container image).  Unlike
the old pack format, a group does **not** reference pre-built archives; it
only names plans.  `wright launch` resolves those plans, builds them, packages
the outputs, and installs everything into a target root.

## Top-Level `[group]` Table

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Group name |
| `version` | string | yes | — | Free-form version string |
| `description` | string | no | `""` | Human-readable description |
| `arch` | string | no | `""` | Target architecture hint |
| `plans` | list of strings | no | `[]` | Plan names to build and install |

## `plans`

The `plans` array lists the plan names that belong to this group.  At launch
time Wright:

1. Discovers each named plan under the configured `plans_dir`.
2. Expands build, link, and runtime dependencies automatically.
3. Computes build waves from the dependency graph.
4. Builds, packages, and installs each wave into the target root.

```toml
[group]
name = "core"
version = "1.0"
plans = ["glibc", "bash", "coreutils", "openssl"]
```

## `[[assume]]` — External Assumptions

Parts that the target system is expected to provide but which Wright did not
install.  Common examples: the kernel on a VPS, or the host toolchain during
an LFS bootstrap.

```toml
[[assume]]
name    = "linux"
version = "6.12.0"
```

Each assumption is recorded in the target database via `wright assume` before
any plans are built, so dependency checks pass.

## `[config]` — Declarative System Configuration

Optional settings applied after all plans are installed.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `hostname` | string | — | Written to `/etc/hostname` |
| `timezone` | string | — | Symlinked to `/etc/localtime` |
| `locale` | string | — | Written to `/etc/locale.conf` |
| `services` | list of strings | `[]` | runit service names; launch creates `/var/service/<name>` symlinks pointing at `/etc/sv/<name>` |

```toml
[config]
hostname = "wright"
timezone = "UTC"
locale   = "en_US.UTF-8"
services = ["sshd", "ntpd"]
```

## Discovery

When `wright launch --plans ./plans @core` is used, Wright searches for the
named group under the plans directory in this order:

1. `./plans/groups/core.toml`
2. `./plans/core/group.toml`

A plans directory may contain any number of group files alongside the actual
plan directories.

## Example

```toml
[group]
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

[config]
hostname = "container"
timezone = "UTC"
```
