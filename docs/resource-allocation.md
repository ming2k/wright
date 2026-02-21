# Resource Allocation

This page explains how `wbuild` allocates CPU time across builds and how to tune each layer.

## Two Layers of Parallelism

| Layer | Controls | Where to configure |
|-------|----------|--------------------|
| **Worker concurrency** | How many packages build simultaneously | `build.workers` in `wright.toml`, overridden by `-w` / `--workers` |
| **CPU affinity** | How many CPUs each sandboxed process can use | Computed as `total_cpus / active_workers`; overridden by `build.nproc_per_worker` in `wright.toml` |

The layers compose. On a 16-core machine with 4 concurrent workers, each sandboxed build process is pinned to 4 CPUs — so `nproc` inside the sandbox returns 4, and `make -j$(nproc)` uses 4 threads. 16 threads total.

## Worker Concurrency

The scheduler runs as many workers as the limit allows, but only launches a package when **all of its dependencies in the current build set have finished**. Dependency ordering is enforced automatically; the worker count is a ceiling, not a guarantee.

```
dependency graph:          with workers = 3:
  A ─┐
  B ─┼─► D ─► F          step 1: A, B, C  (no deps, all launch)
  C ─┘                    step 2: D        (waits for A, B)
                           step 3: E        (waits for C)
  C ──► E                 step 4: F        (waits for D)
```

Setting `workers` higher than the number of packages independent at any given point has no effect — the scheduler finds no additional ready work.

Configure in `wright.toml`:

```toml
[build]
workers = 4   # default: 0 (auto-detect CPU count)
```

Or per-invocation with `--workers` / `-w`:

```bash
wbuild run -w 4 @base
```

## CPU Affinity Isolation

Wright pins each sandboxed build process to its computed CPU share using `sched_setaffinity`. This means tools like `nproc` inside the sandbox return the correct count without any environment variable injection — the kernel enforces it.

The CPU share for each worker is computed as:

```
cpu_share = total_cpus / active_workers
```

`active_workers` is the number of packages actually building at the moment a stage launches — not the workers ceiling. When the graph fans out, each package gets a smaller share; when it collapses to a single runnable package, that package gets the full CPU budget.

**CPU shares are locked when a stage starts.** A stage already running is not re-pinned if another worker finishes mid-flight.

### Static override

If you want a fixed per-worker CPU count instead of the dynamic share, set `nproc_per_worker` in `wright.toml`:

```toml
[build]
workers = 4
nproc_per_worker = 4   # each worker always gets exactly 4 CPUs
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

## Example: 16-core machine

| `workers` | `nproc_per_worker` | Active workers | CPUs per worker |
|-----------|-------------------|----------------|-----------------|
| 0 (→16)  | unset             | 1              | 16              |
| 0 (→16)  | unset             | 4              | 4               |
| 0 (→16)  | unset             | 8              | 2               |
| 4         | unset             | 4              | 4               |
| 4         | 2                 | any            | 2 (fixed)       |
| 1         | unset             | 1              | 16              |

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
