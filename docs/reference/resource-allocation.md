# Resource Allocation

This page explains how `wright build` allocates CPU time across builds.

## Automatic Parallelism

Wright no longer exposes a user-facing `isolations` concurrency setting. Build
task parallelism is chosen automatically from the usable CPU budget, and the
scheduler still respects dependency ordering.

On a 16-core machine with 12 usable CPUs, Wright can run up to 12 independent
build tasks in parallel. If only one task is ready, only one task runs.

## CPU Budget

By default wright reserves 4 CPUs for the OS, keeping the system responsive during heavy parallel builds:

```
total_cpus = available_cpus - 4  (minimum 1)
```

Override with `max_cpus` in `wright.toml`:

```toml
[build]
max_cpus = 16  # use exactly 16 cores; 0 or unset = available - 4
```

## Build Task Concurrency

The scheduler launches a build task only when **all of its dependencies in the
current build set have finished**. Dependency ordering is enforced
automatically. The concurrency limit is internal and derived from the usable
CPU budget, not from a user-configured `isolations` value.

```
dependency graph:
 A ─┐
 B ─┼─► D ─► F     step 1: A, B, C may start together if CPU budget allows
 C ─┘             step 2: D waits for A and B
                  step 3: E waits for C
 C ──► E          step 4: F waits for D
```

Even on a machine with many CPUs, Wright cannot exceed the number of currently
independent tasks in the graph.

## CPU Affinity Isolation

Wright pins each build task process to its computed CPU share using
`sched_setaffinity`. Tools like `nproc` inside the stage return the correct
count without any environment variable injection — the kernel enforces it.

The CPU share for each active build task is computed as:

```
cpu_share = total_cpus / active_tasks
```

`active_tasks` is the number of parts actually building at the moment a stage
launches. When the graph fans out, each part gets a smaller share; when it
collapses to a single runnable part, that part gets the full CPU budget.

**CPU shares are locked when a stage starts.** A stage already running is not re-pinned if another isolation finishes mid-flight.

### Static override

If you want a fixed per-task CPU count instead of the dynamic share, set
`nproc_per_isolation` in `wright.toml`:

```toml
[build]
nproc_per_isolation = 4  # each build task always gets exactly 4 CPUs
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

| `max_cpus` | `nproc_per_isolation` | Active build tasks | CPUs per task |
|------------|----------------------|--------------------|---------------|
| unset      | unset                | 1                  | 12            |
| unset      | unset                | 4                  | 3             |
| unset      | unset                | 6                  | 2             |
| 16         | unset                | 4                  | 4             |
| unset      | 2                    | any                | 2 (fixed)     |

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
timeout = 7200    # 2-hour wall-clock limit per stage
cpu_time_limit = 3600 # 1-hour CPU-time limit

# plan.toml — tighter limits for a known-fast part
[options]
timeout = 300
memory_limit = 2048
```
