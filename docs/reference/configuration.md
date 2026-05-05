# Configuration

## Priority

Without `--config`, Wright loads `wright.toml` in this order:

1. `/etc/wright/wright.toml`
2. `$XDG_CONFIG_HOME/wright/wright.toml` for non-root users
3. `./wright.toml`
4. `WRIGHT_*` environment variables

Higher-priority files override lower-priority ones by key.

With `--config <path>`, Wright loads that file instead of the default file
layers. `WRIGHT_*` environment variables still override it.

## Main Config

```toml
[general]
arch = "x86_64"
plans_dir = "/var/lib/wright/plans"
parts_dir = "/var/lib/wright/parts"
source_dir = "/var/lib/wright/sources"
db_path = "/var/lib/wright/wright.db"
logs_dir = "/var/log/wright"
executors_dir = "/etc/wright/executors"

[build]
build_dir = "/var/tmp/wright/workshop"
default_isolation = "strict"
ccache = false
memory_limit = 8192
cpu_time_limit = 7200
timeout = 14400
nproc_per_isolation = 4
max_cpus = 16
stable_toolchain = ["gcc", "glibc", "binutils", "make", "bison", "flex",
                    "perl", "python", "texinfo", "m4", "sed", "gawk"]

[network]
download_timeout = 300
retry_count = 3
```

## Important Paths

| Field | Default | Meaning |
|------|---------|---------|
| `plans_dir` | `/var/lib/wright/plans` | plan tree root |
| `extra_plans_dirs` | `[]` | additional plan tree roots |
| `parts_dir` | `/var/lib/wright/parts` | local archive store |
| `source_dir` | `/var/lib/wright/sources` | source and git cache |
| `db_path` | `/var/lib/wright/wright.db` | system state database |
| `logs_dir` | `/var/log/wright` | reserved operation log directory |
| `executors_dir` | `/etc/wright/executors` | custom executor directory |
| `build_dir` | `/var/tmp/wright/workshop` | build work directory |
| `default_isolation` | `strict` | default lifecycle isolation |
| `ccache` | `false` | global ccache default |
| `memory_limit` | unset | virtual memory limit in MB |
| `cpu_time_limit` | unset | per-process CPU seconds |
| `timeout` | unset | per-stage wall-clock seconds |
| `nproc_per_isolation` | unset | fixed CPU budget exposed as `NPROC` |
| `max_cpus` | unset | maximum total CPUs Wright may use |
| `stable_toolchain` | (see below) | part names treated as stable for rebuild cascade decisions |
| `download_timeout` | `300` | network timeout in seconds |
| `retry_count` | `3` | download retry count |

## Notes

- `stable_toolchain` lists part names that are never treated as "outdated" when computing dependency rebuild cascades. The default list covers the core LFS bootstrap toolchain (`gcc`, `glibc`, `binutils`, `make`, etc.). Add or replace entries when your distribution uses different package names (e.g. `gcc-14` or `musl`).

- `plans_dir` does not automatically move to a user path; override it explicitly for non-root setups.
- `extra_plans_dirs` are searched after `plans_dir`.
- `parts_dir` is the local stock of built archives.
- `db_path` tracks the authoritative state of installed parts, files, dependencies, and build sessions.
- Lock files live under the Wright lock directory derived from `db_path`, typically `/var/lib/wright/lock/`.
- `source_dir` caches downloaded sources and git repositories.
