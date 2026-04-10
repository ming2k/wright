# Configuration

## Priority

Wright loads `wright.toml` in this order:

1. `--config <path>`
2. `./wright.toml`
3. `$XDG_CONFIG_HOME/wright/wright.toml` for non-root users
4. `/etc/wright/wright.toml`

Higher-priority files override lower-priority ones by key.

## Assemblies

Assemblies live under `assemblies_dir` and use `[[assembly]]` tables:

```toml
[[assembly]]
name = "base"
description = "Base system maintenance set"
plans = ["bash", "coreutils", "grep"]

[[assembly]]
name = "devel"
plans = ["gcc", "make", "pkgconf"]
includes = ["base"]
```

Assemblies are convenience sets, not dependency graphs.

## Main Config

```toml
[general]
arch = "x86_64"
plans_dir = "/var/lib/wright/plans"
components_dir = "/var/lib/wright/components"
cache_dir = "/var/lib/wright/cache"
db_path = "/var/lib/wright/db/parts.db"
inventory_db_path = "/var/lib/wright/db/inventory.db"
log_dir = "/var/log/wright"
executors_dir = "/etc/wright/executors"
assemblies_dir = "/var/lib/wright/assemblies"

[build]
build_dir = "/var/tmp/wright-build"
default_dockyard = "strict"
dockyards = 0
cflags = "-O2 -pipe -march=x86-64"
cxxflags = "-O2 -pipe -march=x86-64"
ccache = false

[network]
download_timeout = 300
retry_count = 3
```

## Important Paths

| Field | Default | Meaning |
|------|---------|---------|
| `plans_dir` | `/var/lib/wright/plans` | plan tree root |
| `components_dir` | `/var/lib/wright/components` | local archive store |
| `db_path` | `/var/lib/wright/db/parts.db` | installed-system DB |
| `inventory_db_path` | `/var/lib/wright/db/inventory.db` | local built-archive inventory |
| `assemblies_dir` | `/var/lib/wright/assemblies` | assembly definition files |
| `build_dir` | `/var/tmp/wright-build` | build work directory |

## Notes

- `plans_dir` does not automatically move to a user path; override it explicitly for non-root setups.
- `components_dir` is just the local stock of built archives.
- `inventory_db_path` tracks local build outputs only. The legacy config key
  `repo_db_path` is still accepted as an alias for migration.
- Lock files live under the Wright lock directory derived from `db_path`, typically `/var/lib/wright/lock/`.
