# Folio Manifest (`<name>.toml`)

Reference for the folio manifest format consumed by `wright launch` and
`wright install`.

A folio is a declarative list of plans that form a coherent system (a
minimal base, a development workstation, a container image, …).  It
contains no pre-built artefacts — only plan names, external assumptions,
and optional post-launch hooks.

Unknown keys and tables are rejected at parse time.

## File Location

Folio files are bare TOML files named after the folio: `core.toml`,
`desktop.toml`, etc.  They live in a dedicated folios directory that is a
**peer** of the plans directory, not nested inside it:

```
/var/lib/wright/
├── plans/        ← plan recipes (per-part)
│   ├── glibc/
│   └── bash/
└── folios/       ← system recipes (per-system)
    ├── core.toml
    └── desktop.toml
```

Resolution order for `@name`:

1. `wright launch --folios <DIR>` (if given) — `<DIR>/<name>.toml`
2. `general.folios_dir` — `<dir>/<name>.toml`

The first match wins.  Folios are never searched under `plans_dir`.

## `[folio]` (required)

| Field         | Type            | Required | Default | Description                                  |
|---------------|-----------------|----------|---------|----------------------------------------------|
| `name`        | string          | yes      | —       | Folio identifier; used in `@name` references |
| `version`     | string          | yes      | —       | Free-form version label                      |
| `description` | string          | no       | `""`    | Human-readable summary                       |
| `plans`       | list of strings | no       | `[]`    | Plan names to forge and deploy               |

`version` is not compared or resolved.  Bump it when the folio's plan list
or configuration changes.

```toml
[folio]
name        = "core"
version     = "1.0"
description = "Minimal usable system"
plans       = ["glibc", "bash", "coreutils", "openssl"]
```

## `[[provide]]` (optional, repeatable)

Parts the target system is expected to provide but which Wright did not
deploy.  Common examples: the kernel on a VPS, the host toolchain during
LFS bootstrap, a pre-installed bootloader.

Each entry is recorded in the target database via `wright provide` before
any plans are built, so dependency checks pass.

| Field     | Type   | Required | Description                          |
|-----------|--------|----------|--------------------------------------|
| `name`    | string | yes      | Part name to assume as provided      |
| `version` | string | yes      | Version string for the provided part |

```toml
[[provide]]
name    = "linux"
version = "6.12.0"
```

## `[[hook]]` (optional, repeatable)

Shell scripts executed after all plans are built and deployed.

| Field    | Type   | Required | Description                                                              |
|----------|--------|----------|--------------------------------------------------------------------------|
| `stage`  | enum   | yes      | Hook stage. Only `"post-launch"` is supported; unknown values are errors |
| `script` | string | yes      | Shell command to execute on the host                                     |

Hooks run on the host under `sh -c` with the same privileges as `wright`.
Both `$WRIGHT_ROOT` and `$ROOT` are set to the target root path.  Hooks
are **not** sandboxed.

```toml
[[hook]]
stage  = "post-launch"
script = """
echo "myhost" > $ROOT/etc/hostname
ln -sf ../usr/share/zoneinfo/UTC $ROOT/etc/localtime
echo "LANG=en_US.UTF-8" > $ROOT/etc/locale.conf
ln -s /etc/sv/sshd $ROOT/var/service/sshd
"""
```

## Full Example

```toml
[folio]
name        = "container-base"
version     = "2026.05"
description = "Minimal container image"
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

[[provide]]
name    = "linux"
version = "6.12.0"

[[hook]]
stage  = "post-launch"
script = "echo container > $ROOT/etc/hostname"
```

## Related

- [How to write a folio](../how-to/write-a-folio.md)
- [Launch design](../explanation/launch-design.md)
