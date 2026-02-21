# Resource Allocation

This page explains how `wbuild` allocates CPU time across builds and how to tune each layer.

## Two Layers of Parallelism

| Layer | Controls | Where to configure |
|-------|----------|--------------------|
| **Dockyard concurrency** | How many packages build simultaneously | `build.dockyards` in `wright.toml`, overridden by `-w` / `--dockyards` |
| **CPU affinity** | How many CPUs each dockyard process can use | Computed as `total_cpus / active_dockyards`; overridden by `build.nproc_per_dockyard` in `wright.toml` |

The layers compose. On a 16-core machine (12 usable after the 4-core OS reserve) with 4 concurrent dockyards, each dockyard process is pinned to 3 CPUs — so `nproc` inside the dockyard returns 3, and `make -j$(nproc)` uses 3 threads. 12 threads total.

## CPU Budget

By default wright reserves 4 CPUs for the OS, keeping the system responsive during heavy parallel builds:

```
total_cpus = available_cpus - 4   (minimum 1)
```

Override with `max_cpus` in `wright.toml`:

```toml
[build]
max_cpus = 16   # use exactly 16 cores; 0 or unset = available - 4
```

## Dockyard Concurrency

The scheduler runs as many dockyards as the limit allows, but only launches a package when **all of its dependencies in the current build set have finished**. Dependency ordering is enforced automatically; the dockyard count is a ceiling, not a guarantee.

```
dependency graph:          with dockyards = 3:
  A ─┐
  B ─┼─► D ─► F          step 1: A, B, C  (no deps, all launch)
  C ─┘                    step 2: D        (waits for A, B)
                           step 3: E        (waits for C)
  C ──► E                 step 4: F        (waits for D)
```

Setting `dockyards` higher than the number of packages independent at any given point has no effect — the scheduler finds no additional ready work.

Configure in `wright.toml`:

```toml
[build]
dockyards = 4   # default: 0 (auto = total_cpus)
```

Or per-invocation with `--dockyards` / `-w`:

```bash
wbuild run -w 4 @base
```

## CPU Affinity Isolation

Wright pins each dockyard process to its computed CPU share using `sched_setaffinity`. Tools like `nproc` inside the dockyard return the correct count without any environment variable injection — the kernel enforces it.

The CPU share for each dockyard is computed as:

```
cpu_share = total_cpus / active_dockyards
```

`active_dockyards` is the number of packages actually building at the moment a stage launches — not the dockyards ceiling. When the graph fans out, each package gets a smaller share; when it collapses to a single runnable package, that package gets the full CPU budget.

**CPU shares are locked when a stage starts.** A stage already running is not re-pinned if another dockyard finishes mid-flight.

### Static override

If you want a fixed per-dockyard CPU count instead of the dynamic share, set `nproc_per_dockyard` in `wright.toml`:

```toml
[build]
dockyards = 4
nproc_per_dockyard = 4   # each dockyard always gets exactly 4 CPUs
```

### Per-plan control

Scripts own their parallelism entirely. Since `nproc` returns the correct CPU count (enforced by affinity), scripts should use it directly:

```bash
make -j$(nproc)
ninja -j$(nproc)
```

For builds that need a different strategy — serial, half the cores, a fixed number — use `[options.env]` to set `MAKEFLAGS` (or whatever the tool reads) explicitly:

```toml
# Serial build
[options]
env = { MAKEFLAGS = "-j1" }

# Fixed thread count
[options]
env = { MAKEFLAGS = "-j4" }

# Go: bound runtime and build parallelism
[options]
env = { GOFLAGS = "-p=$(nproc)", GOMAXPROCS = "$(nproc)" }
```

## Example: 16-core machine (12 usable, OS reserve = 4)

| `max_cpus` | `dockyards` | `nproc_per_dockyard` | Active dockyards | CPUs per dockyard |
|------------|-------------|----------------------|------------------|-------------------|
| unset      | 0 (→12)    | unset                | 1                | 12                |
| unset      | 0 (→12)    | unset                | 4                | 3                 |
| unset      | 0 (→12)    | unset                | 6                | 2                 |
| 16         | 0 (→16)    | unset                | 4                | 4                 |
| unset      | 4           | unset                | 4                | 3                 |
| unset      | 4           | 2                    | any              | 2 (fixed)         |
| unset      | 1           | unset                | 1                | 12                |

## Memory and Time Limits

Wright can enforce hard resource limits per build stage via the kernel's `rlimit` interface. Off by default.

| Setting | Where | Effect |
|---------|-------|--------|
| `memory_limit` (MB) | `wright.toml [build]` or `plan.toml [options]` | Caps virtual address space (`RLIMIT_AS`). Set generously (2–3× expected peak) — rustc and Go reserve large virtual mappings they never fully commit. |
| `cpu_time_limit` (seconds) | same | Caps aggregate CPU time for the stage process tree (`RLIMIT_CPU`). Kills runaway compiler loops. |
| `timeout` (seconds) | same | Wall-clock deadline per stage. Kills the process group when elapsed, regardless of CPU usage. |

Per-plan values take precedence over global config:

```toml
# wright.toml — global safety nets
[build]
timeout = 7200        # 2-hour wall-clock limit per stage
cpu_time_limit = 3600 # 1-hour CPU-time limit

# plan.toml — tighter limits for a known-fast package
[options]
timeout = 300
memory_limit = 2048
```
