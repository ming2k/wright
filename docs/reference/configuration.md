# Configuration

## Priority

Wright loads `wright.toml` in this order:

1. `--config <path>`
2. `./wright.toml`
3. `$XDG_CONFIG_HOME/wright/wright.toml` for non-root users
4. `/etc/wright/wright.toml`

Higher-priority files override lower-priority ones by key.

## Main Config

```toml
[general]
arch = "x86_64"
plans_dir = "/var/lib/wright/plans"
parts_dir = "/var/lib/wright/parts"
source_dir = "/var/lib/wright/sources"
db_path = "/var/lib/wright/wright.db"
log_dir = "/var/logs/wright"
executors_dir = "/etc/wright/executors"

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
| `db_path` | `/var/lib/wright/wright.db` | system state database |
| `build_dir` | `/var/tmp/wright/workshop` | build work directory |

## Notes

- `plans_dir` does not automatically move to a user path; override it explicitly for non-root setups.
- `parts_dir` is the local stock of built archives.
- `db_path` tracks the authoritative state of installed parts, files, dependencies, and build sessions.
- Lock files live under the Wright lock directory derived from `db_path`, typically `/var/lib/wright/lock/`.
- `source_dir` caches downloaded sources and git repositories.
