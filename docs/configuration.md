# Configuration

## Configuration Priority

Wright uses a **layered merge** strategy for `wright.toml`. All config files
that exist are loaded and merged in priority order — a higher-priority file
only needs to set the keys it wants to override; every other key is
transparently inherited from the layer below it.

If no file is found at any location, built-in defaults are applied.

The `--config` CLI flag bypasses layering entirely and loads that single file
with no merging.

### wright.toml (global config)

| Priority | Path | When |
|----------|------|------|
| 1 (highest) | `--config <path>` | Explicit CLI override — no layering |
| 2 | `./wright.toml` | Project-local overrides |
| 3 | `$XDG_CONFIG_HOME/wright/wright.toml` | Per-user overrides (non-root only) |
| 4 (lowest) | `/etc/wright/wright.toml` | System-wide base config |

All fields have defaults, so every config file is optional. A user config only
needs to contain the keys it wants to change — the rest come from the system
config or built-in defaults.

### repos.toml (repository sources)

| Priority | Path |
|----------|------|
| 1 | `/etc/wright/repos.toml` |

Repository configuration is system-wide only. Within a single `repos.toml`,
sources are ranked by the `priority` field (higher number = preferred).

### assembly definitions

Assemblies group **plans** for batch building with `wbuild run @name`.
Kits group **parts** for batch installation with `wright install @name`.

Both are **non-dependent, combinatory groupings** — membership in an assembly or
kit implies no dependency relationship between the items. The items are
independent units that happen to be bundled together for convenience (like a
kit of parts for a build). Actual dependency resolution is handled separately
by the build system and system manager.

This means assemblies and kits are freely composable: you can combine
multiple groups in one command (`wbuild run @core @devel`, `wright install @base @gui`),
and overlapping members are simply deduplicated.

Assembly definitions are loaded from all `*.toml` files in `assemblies_dir`
(default: `/var/lib/wright/assemblies/`). Each file can contain multiple
assemblies using `[[assembly]]` array-of-tables syntax. The filename groups
related assemblies logically.

```toml
# /var/lib/wright/assemblies/qemu.toml
[[assembly]]
name = "qemu-base"
description = "Core QEMU system emulator part."
plans = ["qemu"]

[[assembly]]
name = "qemu-firmware"
description = "Firmware set commonly used with QEMU PC guests."
plans = ["seabios"]

[[assembly]]
name = "qemu-network"
description = "User-mode networking support for QEMU."
plans = ["libslirp"]
```

### kits (part groups)

Kit definitions are loaded from all `*.toml` files in `kits_dir`
(default: `/var/lib/wright/kits/`). Each file can contain multiple
kits using `[[kit]]` array-of-tables syntax, consistent with
assemblies and `[[source]]`.

```toml
# /var/lib/wright/kits/base.toml
[[kit]]
name = "base"
description = "Base system parts"
parts = ["glibc", "coreutils", "bash", "libgcc", "libstdc++"]

[[kit]]
name = "devel"
description = "Development tools"
parts = ["gcc", "binutils", "make"]
includes = ["base"]   # inherit all part names from @base
```

### executor definitions

Executors are always loaded from `{executors_dir}/*.toml` (default:
`/etc/wright/executors/`). The directory path itself follows `wright.toml`
priority since it is a `[general]` config field.

### Summary

```
defaults                     ← built-in fallback (always present)
  └─ /etc/wright/            ← system-wide base (merged on top)
      └─ ~/.config/wright/   ← per-user overrides (merged on top, non-root)
          └─ ./wright.toml   ← project-local overrides (merged on top)
              └─ --config    ← explicit path, bypasses all layering
```

Each layer only needs to contain the keys it wants to change. Keys absent
from a layer are inherited from the layer below.

## wright.toml

### Default Paths

| Use Case | Config | Cache | Database |
|----------|--------|-------|----------|
| **System (root)** | `/etc/wright/wright.toml` | `/var/lib/wright/cache` | `/var/lib/wright/db/parts.db` |
| **User (non-root)** | `~/.config/wright/wright.toml` | `~/.cache/wright` | `/var/lib/wright/db/parts.db` |

```toml
[general]
arch = "x86_64"                         # System architecture
plans_dir = "/var/lib/wright/plans"      # Plan definitions
components_dir = "/var/lib/wright/components" # Built part archives
cache_dir = "/var/lib/wright/cache"       # Downloaded sources cache
db_path = "/var/lib/wright/db/parts.db" # Installed part database
repo_db_path = "/var/lib/wright/db/repo.db" # Repository index database
log_dir = "/var/log/wright"               # Operation logs (build logs are under build_dir/<name>-<version>/log)
executors_dir = "/etc/wright/executors"   # Executor definitions (*.toml)
assemblies_dir = "/var/lib/wright/assemblies" # Assembly definitions (*.toml)
kits_dir = "/var/lib/wright/kits"             # Kit (part group) definitions (*.toml)
repo_dir = "/var/lib/wright/repo"             # Repository directory for indexes and imported archives

[build]
build_dir = "/var/tmp/wright-build"       # Build working directory
default_dockyard = "strict"             # Default dockyard level: none / relaxed / strict
dockyards = 0                           # Max concurrent dockyards (0 = auto = available_cpus - 4, minimum 1)
# nproc_per_dockyard = 4               # Fixed CPU count per dockyard (unset = dynamic share)
# max_cpus = 16                        # Hard cap on CPU cores used (0 or unset = available - 4)
cflags = "-O2 -pipe -march=x86-64"     # Default C compiler flags
cxxflags = "-O2 -pipe -march=x86-64"   # Default C++ compiler flags
ccache = false                          # Enable ccache
# memory_limit = 8192                   # Max virtual address space per build process (MB)
# cpu_time_limit = 7200                 # Max CPU time per build process (seconds)
# timeout = 14400                       # Wall-clock timeout per build stage (seconds)

[network]
download_timeout = 300                  # Download timeout in seconds
retry_count = 3                         # Download retry attempts
```

`plans_dir` does not automatically switch to a per-user location. For
non-root use, point it at a writable plan tree such as `~/wright/plans` in
your user config:

```toml
[general]
plans_dir = "/home/alice/wright/plans"
```

### `[general]` section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `arch` | string | `"x86_64"` | Target architecture |
| `plans_dir` | path | `/var/lib/wright/plans` | Root directory for plan definitions. Override this for non-root use if your plans live outside `/var/lib/wright/plans`. |
| `components_dir` | path | `/var/lib/wright/components` | Built part archives (`.wright.tar.zst`) |
| `cache_dir` | path | `/var/lib/wright/cache` | Downloaded sources cache |
| `db_path` | path | `/var/lib/wright/db/parts.db` | Installed part database (SQLite) |
| `repo_db_path` | path | `/var/lib/wright/db/repo.db` | Repository index database (SQLite) |
| `log_dir` | path | `/var/log/wright` | Operation logs (build logs live under `build_dir/<name>-<version>/log`) |
| `executors_dir` | path | `/etc/wright/executors` | Executor definition files (`*.toml`) |
| `assemblies_dir` | path | `/var/lib/wright/assemblies` | Assembly definition files (`*.toml`) |
| `kits_dir` | path | `/var/lib/wright/kits` | Kit (part group) definition files (`*.toml`) |
| `repo_dir` | path | `/var/lib/wright/repo` | Repository directory used for indexes and archive scanning |

### `[build]` section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `build_dir` | path | `/var/tmp/wright-build` | Working directory for builds. Prefer a large persistent filesystem over `/tmp`. |
| `default_dockyard` | string | `"strict"` | Dockyard isolation level when not specified per-stage (`none` / `relaxed` / `strict`) |
| `dockyards` | integer | `0` | Max concurrent dockyard processes. `0` = auto (available_cpus − 4, minimum 1). |
| `nproc_per_dockyard` | integer | — | Fixed CPU count per dockyard. Unset = dynamic (`total_cpus / active_dockyards`). |
| `max_cpus` | integer | — | Hard cap on total CPU cores wright may use. Unset = `available_cpus - 4` (minimum 1). |
| `cflags` | string | `"-O2 -pipe -march=x86-64"` | Global C compiler flags |
| `cxxflags` | string | `"-O2 -pipe -march=x86-64"` | Global C++ compiler flags |
| `ccache` | boolean | `false` | Use ccache if available |
| `memory_limit` | integer | — | Max virtual address space per build process (MB). Uses `RLIMIT_AS`. Set generously — see note below. |
| `cpu_time_limit` | integer | — | Max CPU time per build process (seconds). Uses `RLIMIT_CPU`. |
| `timeout` | integer | — | Wall-clock timeout per build stage (seconds). Kills the process on expiry. |

### `[network]` section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `download_timeout` | integer | `300` | HTTP download timeout (seconds) |
| `retry_count` | integer | `3` | Number of retry attempts for failed downloads |

## repos.toml

Repository source configuration at `/etc/wright/repos.toml`. Defines where wright looks for parts.

```toml
# Local hold tree
[[source]]
name = "hold"
type = "hold"
path = "/var/hold"
priority = 100

# Remote binary repository
[[source]]
name = "official"
type = "remote"
url = "https://repo.example.com/x86_64"
priority = 200
gpg_key = "/etc/wright/keys/official.gpg"
enabled = true

# Local binary part cache
[[source]]
name = "local"
type = "local"
path = "/var/lib/wright/cache/parts"
priority = 300
```

### Source fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique source identifier |
| `type` | string | `"hold"`, `"remote"`, or `"local"` |
| `path` | path | Local path (for `hold` and `local` types) |
| `url` | string | Repository URL (for `remote` type) |
| `priority` | integer | Higher number = preferred when multiple sources have the same part |
| `gpg_key` | path | GPG public key for signature verification (optional) |
| `enabled` | boolean | Whether this source is active (default: `true`) |

> **Note:** Remote repository support (sync, download, signature verification) is planned for Phase 4.

## Executor Definitions

Executors define how build scripts are run. They live in `/etc/wright/executors/` as TOML files.

### Shell executor

```toml
# /etc/wright/executors/shell.toml
[executor]
name = "shell"
description = "Bash shell executor"
command = "/bin/bash"
args = ["-e", "-o", "pipefail"]
delivery = "tempfile"
tempfile_extension = ".sh"
required_paths = ["/bin", "/usr/bin"]
default_dockyard = "strict"
```

### Python executor

```toml
# /etc/wright/executors/python.toml
[executor]
name = "python"
description = "Python 3 executor"
command = "/usr/bin/python3"
args = ["-u"]
delivery = "tempfile"
tempfile_extension = ".py"
required_paths = ["/usr/lib/python3", "/usr/lib/python3.*/"]
default_dockyard = "strict"
```

### Lua executor

```toml
# /etc/wright/executors/lua.toml
[executor]
name = "lua"
description = "Lua 5.4 executor"
command = "/usr/bin/lua"
args = []
delivery = "tempfile"
tempfile_extension = ".lua"
required_paths = ["/usr/lib/lua", "/usr/share/lua"]
default_dockyard = "strict"
```

### Executor fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Executor identifier (referenced in `plan.toml` lifecycle stages) |
| `description` | string | Human-readable description |
| `command` | path | Absolute path to the interpreter binary |
| `args` | string[] | Arguments passed to the interpreter |
| `delivery` | string | How scripts are passed: `"tempfile"` (write to file, pass path) or `"stdin"` (pipe via stdin) |
| `tempfile_extension` | string | File extension for temp scripts (used with `tempfile` delivery) |
| `required_paths` | string[] | Additional paths to bind-mount read-only inside the dockyard |
| `default_dockyard` | string | Default dockyard isolation level if not specified by the plan stage |

### Custom executors

Add new executors by placing TOML files in `/etc/wright/executors/`. The `command` must be an absolute path to an existing executable. No shell metacharacters or pipes.
