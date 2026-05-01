# Configuration

## Priority

Wright loads `wright.toml` in this order:

1. `--config <path>`
2. `./wright.toml`
3. `$XDG_CONFIG_HOME/wright/wright.toml` for non-root users
4. `/etc/wright/wright.toml`

Higher-priority files override lower-priority ones by key.
## Assemblies

Assemblies live under `assemblies_dir` and allow you to group plans together. For a detailed guide, see [Writing Assemblies](writing-assemblies.md).

`base.toml`:
```toml
name = "base"
description = "Base system maintenance set"
plans = ["bash", "coreutils", "grep"]
```

`devel.toml`:
```toml
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
parts_dir = "/var/lib/wright/parts"
source_dir = "/var/lib/wright/sources"
installed_db_path = "/var/lib/wright/state/installed.db"
archive_db_path = "/var/lib/wright/state/archives.db"
log_dir = "/var/logs/wright"
executors_dir = "/etc/wright/executors"
assemblies_dir = "/var/lib/wright/assemblies"

[build]
build_dir = "/var/tmp/wright/workshop"
default_isolation = "strict"
ccache = false

[network]
download_timeout = 300
retry_count = 3
```

## Important Paths

| Field | Default | Meaning |
|------|---------|---------|
| `plans_dir` | `/var/lib/wright/plans` | plan tree root |
| `parts_dir` | `/var/lib/wright/parts` | local archive store |
| `installed_db_path` | `/var/lib/wright/state/installed.db` | installed-system DB |
| `archive_db_path` | `/var/lib/wright/state/archives.db` | local built-archive catalogue |
| `assemblies_dir` | `/var/lib/wright/assemblies` | assembly definition files |
| `build_dir` | `/var/tmp/wright/workshop` | build work directory |

## Notes

- `plans_dir` does not automatically move to a user path; override it explicitly for non-root setups.
- `parts_dir` is just the local stock of built archives.
- `archive_db_path` tracks local build outputs only. Legacy keys `inventory_db_path` and
  `repo_db_path` are still accepted as aliases for migration.
- `installed_db_path` is a snapshot of what is currently installed on the system. Archives may be
  pruned after installation; the installed DB remains the authoritative source of installed state.
- Lock files live under the Wright lock directory derived from `installed_db_path`, typically `/var/lib/wright/lock/`.
